//! Opt-in live validation through a loopback SSH proxy. The proxy injects auth on
//! the remote server; this process never receives or stores the account key.
use erebine::{json, Client, Routing, Surface, Value};
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
    let models = client.inference().models().send().await?;
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
    let chat = client.inference().chat(&request)?.send().await?;
    let body: Value = chat.json()?;
    assert!(body["choices"].is_array());
    println!(
        "chat: HTTP {}, choices decoded, request_id={}",
        chat.status,
        chat.request_id().is_some()
    );
    let request = json!({"model":model,"input":"Reply only with SDK_OK.","max_output_tokens":64,"store":false});
    let response = client.inference().responses(&request)?.send().await?;
    let body: Value = response.json()?;
    assert!(body["output"].is_array());
    println!("responses: HTTP {}, output decoded", response.status);
    let request = json!({"model":model,"messages":[{"role":"user","content":"Reply only with SDK_OK."}],"max_tokens":64,"store":false,"stream":true,"stream_options":{"include_usage":true}});
    let mut stream = client
        .inference()
        .stream_chat(&request)?
        .header("x-erebine-vendor-events", "on")?
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
    let retrieved = client.inference().model(model).send().await?;
    assert_eq!(retrieved.json::<Value>()?["id"], model);
    println!("model retrieval: HTTP {}", retrieved.status);
    let schema = json!({"type":"json_schema","json_schema":{"name":"sdk_answer","strict":true,"schema":{"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}}});
    let body = json!({"model":model,"messages":[{"role":"user","content":"Return an object with ok set to true."}],"max_tokens":128,"reasoning_effort":"minimal","response_format":schema,"store":false});
    let result: Value = client.inference().chat(&body)?.send().await?.json()?;
    let structured: Value = serde_json::from_str(&erebine::types::output_text(&result))?;
    assert_eq!(structured["ok"], true);
    println!("structured output: JSON schema result decoded");
    let tool = json!({"type":"function","function":{"name":"sdk_echo","description":"Echo the provided synthetic text locally.","parameters":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false},"strict":true}});
    let user = json!({"role":"user","content":"Call sdk_echo with the text sdk-fixture."});
    let body = json!({"model":model,"messages":[user.clone()],"tools":[tool],"tool_choice":{"type":"function","function":{"name":"sdk_echo"}},"max_tokens":128,"reasoning_effort":"minimal","store":false});
    let result: Value = client.inference().chat(&body)?.send().await?.json()?;
    let assistant = &result["choices"][0]["message"];
    let calls = assistant["tool_calls"]
        .as_array()
        .ok_or("tool calls missing")?;
    assert!(!calls.is_empty());
    let mut messages = vec![user, assistant.clone()];
    for call in calls {
        assert_eq!(call["function"]["name"], "sdk_echo");
        let arguments: Value = serde_json::from_str(
            call["function"]["arguments"]
                .as_str()
                .ok_or("tool arguments missing")?,
        )?;
        let message = erebine::types::Message::tool(
            call["id"].as_str().ok_or("tool call id missing")?,
            arguments["text"].as_str().ok_or("tool text missing")?,
        );
        messages.push(serde_json::to_value(message)?);
    }
    let follow = json!({"model":model,"messages":messages,"max_tokens":128,"reasoning_effort":"minimal","store":false});
    let follow: Value = client.inference().chat(&follow)?.send().await?.json()?;
    assert!(!erebine::types::output_text(&follow).is_empty());
    println!("tool calling: function arguments decoded and local result round-tripped");
    if let Some(embedding) = config["embedding_model"].as_str() {
        for format in ["float", "base64"] {
            let request = json!({"model":embedding,"input":["SDK fixture one","SDK fixture two"],"encoding_format":format});
            let value: Value = client
                .inference()
                .embeddings(&request)?
                .send()
                .await?
                .json()?;
            assert_eq!(
                value["data"]
                    .as_array()
                    .ok_or("embedding data missing")?
                    .len(),
                2
            );
            if format == "float" {
                assert!(value["data"][0]["embedding"].is_array());
            } else {
                assert!(value["data"][0]["embedding"].is_string());
            }
        }
        println!("embeddings: batched float and base64 responses decoded");
    }
    if let Some(ranker) = config["rerank_model"].as_str() {
        let request = erebine::types::RerankRequest::new(
            ranker,
            "Rust SDK",
            vec![json!("A Rust client library"), json!("A cooking recipe")],
        );
        let value: Value = client.inference().rerank(&request)?.send().await?.json()?;
        assert_eq!(
            value["results"]
                .as_array()
                .ok_or("rerank results missing")?
                .len(),
            2
        );
        let request = erebine::types::ScoreRequest::new(
            ranker,
            "Rust SDK",
            vec!["A Rust client library".into(), "A cooking recipe".into()],
        );
        let value: Value = client.inference().score(&request)?.send().await?.json()?;
        assert_eq!(
            value["data"].as_array().ok_or("score data missing")?.len(),
            2
        );
        println!("rerank and score: both result formats decoded");
    }
    if let Some(transcriber) = config["transcription_model"].as_str() {
        let mut wav = Vec::new();
        let size = 8000u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + size).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&32000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&size.to_le_bytes());
        wav.resize(44 + size as usize, 0);
        let request = erebine::types::AudioTranscriptionRequest::new(
            transcriber,
            erebine::types::AudioFile::new("sdk-silence.wav", "audio/wav", wav),
        );
        let value: Value = client
            .inference()
            .transcribe(&request)?
            .send()
            .await?
            .json()?;
        assert!(value["text"].is_string());
        println!("transcription: multipart audio accepted, transcript shape decoded");
    }
    Ok(())
}
