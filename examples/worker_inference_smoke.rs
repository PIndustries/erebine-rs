//! Opt-in inference checks against an isolated, pre-enrolled temporary CPU worker.
use erebine::{json, Client, Routing, Surface, Value};
use futures_util::StreamExt;
async fn run(checks: &mut Vec<Value>) -> Result<(), Box<dyn std::error::Error>> {
    let origin = std::env::var("EREBINE_MANAGEMENT_PROXY")?;
    if !origin.starts_with("http://127.0.0.1:") {
        return Err("loopback proxy required".into());
    }
    let bootstrap = Client::new(&origin, Surface::Router)?;
    let cfg: Value = bootstrap
        .request("GET", "/__management/config")
        .query("client", "rust")
        .send()
        .await?
        .json()?;
    let management = bootstrap.project(cfg["project_id"].as_str().ok_or("missing project")?);
    let workspace:Value=management.api().post_project_id_v1_workspaces().json(&json!({"name":format!("{}-chat",cfg["prefix"].as_str().unwrap()),"description":"Isolated native SDK inference fixture"}))?.send().await?.json()?;
    let workspace_id = workspace["id"].as_str().ok_or("missing workspace")?;
    let chat: Value = management
        .api()
        .post_project_id_v1_chats()
        .json(
            &json!({"title":cfg["prefix"],"workspace_id":workspace_id,"model_id":cfg["model_id"]}),
        )?
        .send()
        .await?
        .json()?;
    let chat_id = chat["id"].as_str().ok_or("missing chat")?;
    checks.push(json!({"name":"create isolated chat workspace and chat","result":"passed"}));
    let client = management
        .clone()
        .routing(Routing::Endpoint(
            cfg["endpoint_slug"].as_str().unwrap().into(),
        ))
        .header("x-workspace-id", workspace_id)?
        .header("x-chat-id", chat_id)?;
    let models: Value = client.inference().models().send().await?.json()?;
    if !models["data"].is_array() {
        return Err("model discovery contract".into());
    }
    checks.push(json!({"name":"discover served model","result":"passed"}));
    let body = json!({"model":cfg["model_id"],"messages":[{"role":"user","content":"Synthetic native SDK live test."}],"max_tokens":8,"temperature":0,"store":true,"metadata":{"sdk_validation":"true","chat_id":chat_id}});
    let completed: Value = client.inference().chat(&body)?.send().await?.json()?;
    if completed["choices"].as_array().is_none_or(|v| v.is_empty()) {
        return Err("chat completion contract".into());
    }
    checks.push(json!({"name":"real CPU chat completion","result":"passed"}));
    if let Some(id) = completed["id"].as_str() {
        let _: Value = client
            .api()
            .get_project_id_endpoint_slug_v1_chat_completions_id(id)
            .send()
            .await?
            .json()?;
        let _: Value = client
            .api()
            .get_project_id_endpoint_slug_v1_chat_completions_id_messages(id)
            .send()
            .await?
            .json()?;
        let _: Value = client
            .api()
            .post_project_id_endpoint_slug_v1_chat_completions_id(id)
            .json(&json!({"metadata":{"sdk_validation":"updated"}}))?
            .send()
            .await?
            .json()?;
        client
            .api()
            .delete_project_id_endpoint_slug_v1_chat_completions_id(id)
            .send()
            .await?;
        checks.push(
            json!({"name":"stored completion retrieve messages update delete","result":"passed"}),
        );
    }
    let mut stream=client.inference().stream_chat(&json!({"model":cfg["model_id"],"messages":[{"role":"user","content":"Synthetic stream fixture."}],"max_tokens":8,"store":false,"stream_options":{"include_usage":true}}))?.stream().await?;
    let mut accumulator = erebine::accumulator::ChatAccumulator::default();
    let mut events = 0;
    while let Some(event) = stream.next().await {
        let event = event?;
        accumulator.push(&event)?;
        events += 1;
    }
    if events == 0 || accumulator.choices.is_empty() {
        return Err("SSE completion contract".into());
    }
    checks.push(json!({"name":"real CPU chat SSE and native assembly","result":"passed","events":events,"usage_received":accumulator.usage.is_some()}));
    let response:Value=client.inference().responses(&json!({"model":cfg["model_id"],"input":"Synthetic response fixture.","max_output_tokens":32,"store":true}))?.send().await?.json()?;
    if !response["output"].is_array() {
        return Err("Responses contract".into());
    }
    checks.push(json!({"name":"real CPU Responses generation","result":"passed"}));
    if let Some(id) = response["id"].as_str() {
        let _: Value = client
            .api()
            .get_project_id_endpoint_slug_v1_responses_id(id)
            .send()
            .await?
            .json()?;
        let _: Value = client
            .api()
            .get_project_id_endpoint_slug_v1_responses_id_input_items(id)
            .send()
            .await?
            .json()?;
        client
            .api()
            .delete_project_id_endpoint_slug_v1_responses_id(id)
            .send()
            .await?;
        checks
            .push(json!({"name":"stored Responses retrieve input items delete","result":"passed"}));
    }
    let mut stream=client.inference().stream_responses(&json!({"model":cfg["model_id"],"input":"Synthetic streamed response fixture.","max_output_tokens":32,"store":false}))?.stream().await?;
    let mut count = 0;
    while let Some(event) = stream.next().await {
        let value: Value = event?.json()?;
        if value.get("error").is_some() {
            return Err("response stream error".into());
        }
        count += 1;
    }
    if count == 0 {
        return Err("empty Responses stream".into());
    }
    checks.push(json!({"name":"real CPU Responses SSE","result":"passed","events":count}));
    Ok(())
}
#[tokio::main]
async fn main() {
    let mut checks = vec![];
    let ok = run(&mut checks).await.is_ok();
    println!("{}", json!({"sdk":"rust","checks":checks,"completed":ok}));
    if !ok {
        eprintln!("Worker inference validation did not complete; credentials, identifiers and response bodies were not logged.");
        std::process::exit(1);
    }
}
