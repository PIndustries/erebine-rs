//! Opt-in live validation through a loopback SSH proxy. The proxy injects auth on
//! the remote server; this process never receives or stores the account key.
use erebine::{json, operations, Client, Routing, Surface, Value};
use futures_util::StreamExt;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let origin = std::env::var("EREBINE_VALIDATION_ORIGIN")?;
    if !origin.starts_with("http://127.0.0.1:") {
        return Err("validation proxy must use loopback".into());
    }
    let bootstrap = Client::new(&origin, Surface::Router)?;
    let config: Value = bootstrap
        .request("GET", "/__validation/config")
        .send()
        .await?
        .json()?;
    let client = bootstrap
        .project(config["project"].as_str().ok_or("project missing")?)
        .routing(Routing::Direct {
            workspace: Some(
                config["workspace"]
                    .as_str()
                    .ok_or("workspace missing")?
                    .into(),
            ),
        });
    let model = config["model"].as_str().ok_or("model missing")?;
    let mut mcp = erebine::mcp::McpSession::connect(client.clone()).await?;
    let tools = mcp.list_tools(None).await?;
    let count = tools["tools"].as_array().ok_or("MCP tools missing")?.len();
    assert!(count > 0);
    mcp.close().await?;
    println!("MCP: session initialized, {count} tools discovered, session closed");
    if std::env::var_os("EREBINE_VALIDATION_MCP_ONLY").is_some() {
        return Ok(());
    }
    let models = client.operation(operations::MODELS_LIST).send().await?;
    let list: Value = models.json()?;
    assert!(list["data"]
        .as_array()
        .ok_or("model list missing")?
        .iter()
        .any(|m| m["id"] == model));
    println!(
        "models: HTTP {}, configured model discovered",
        models.status
    );
    let request = json!({"model":model,"messages":[{"role":"user","content":"Reply only with SDK_OK."}],"max_tokens":64,"store":false});
    let chat = client
        .operation(operations::CHAT_COMPLETIONS_CREATE)
        .json(&request)?
        .send()
        .await?;
    let body: Value = chat.json()?;
    assert!(body["choices"].is_array());
    println!(
        "chat: HTTP {}, choices decoded, request_id={}",
        chat.status,
        chat.request_id().is_some()
    );
    let request = json!({"model":model,"input":"Reply only with SDK_OK.","max_output_tokens":64,"store":false});
    let response = client
        .operation(operations::RESPONSES_CREATE)
        .json(&request)?
        .send()
        .await?;
    let body: Value = response.json()?;
    assert!(body["output"].is_array());
    println!("responses: HTTP {}, output decoded", response.status);
    let request = json!({"model":model,"messages":[{"role":"user","content":"Reply only with SDK_OK."}],"max_tokens":64,"store":false,"stream":true,"stream_options":{"include_usage":true}});
    let mut stream = client
        .operation(operations::CHAT_COMPLETIONS_CREATE)
        .header("x-erebine-vendor-events", "on")?
        .json(&request)?
        .stream()
        .await?;
    let mut count = 0;
    let mut usage = false;
    while let Some(event) = stream.next().await {
        let event = event?;
        let body = event.json()?;
        assert!(body.get("error").is_none(), "server streaming error");
        usage |= body.get("usage").is_some_and(|v| !v.is_null());
        count += 1;
    }
    assert!(count > 0);
    println!("stream: {count} JSON events decoded, usage={usage}");
    Ok(())
}
