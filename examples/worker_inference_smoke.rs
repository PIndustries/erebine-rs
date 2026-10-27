//! Opt-in inference checks against an isolated, pre-enrolled temporary CPU worker.
use erebine::{json, Client, Routing, Surface, Value};
use futures_util::StreamExt;
fn validate_response(value: &Value) -> Result<(), Box<dyn std::error::Error>> {
    if !matches!(value["status"].as_str(), Some("completed" | "incomplete"))
        || value["output"].as_array().is_none_or(Vec::is_empty)
        || value["usage"]["output_tokens"].as_u64().unwrap_or(0) == 0
        || !value["error"].is_null()
    {
        return Err("Responses generation contract".into());
    }
    Ok(())
}
async fn check<F>(checks: &mut Vec<Value>, name: &str, operation: F)
where
    F: std::future::Future<Output = Result<Value, Box<dyn std::error::Error>>>,
{
    let record = match operation.await {
        Ok(fields) => {
            let mut value = fields;
            value["name"] = json!(name);
            value["result"] = json!("passed");
            value
        }
        Err(error) => match error.downcast_ref::<erebine::Error>() {
            Some(erebine::Error::Api { status, .. }) => {
                json!({"name":name,"result":"http_failed","status":status})
            }
            _ => json!({"name":name,"result":"response_contract_failed"}),
        },
    };
    checks.push(record);
}
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
    if std::env::var("EREBINE_WORKER_INFERENCE_MODE").as_deref() != Ok("responses") {
        let body = json!({"model":cfg["model_id"],"messages":[{"role":"user","content":"Synthetic native SDK live test."}],"max_tokens":8,"temperature":0,"store":true,"metadata":{"sdk_validation":"true","chat_id":chat_id}});
        let completed: Value = client.inference().chat(&body)?.send().await?.json()?;
        let generated = completed["choices"][0]["message"]["content"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
            && completed["usage"]["completion_tokens"]
                .as_u64()
                .unwrap_or(0)
                > 0;
        checks.push(json!({"name":"real CPU chat completion","result":if generated {"passed"}else{"response_contract_failed"},"output_tokens":completed["usage"]["completion_tokens"]}));
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
        let generated = events > 0
            && accumulator.choices.values().any(|c| !c.content.is_empty())
            && accumulator
                .usage
                .as_ref()
                .and_then(|v| v["completion_tokens"].as_u64())
                .unwrap_or(0)
                > 0;
        checks.push(json!({"name":"real CPU chat SSE and native assembly","result":if generated {"passed"}else{"response_contract_failed"},"events":events,"usage_received":accumulator.usage.is_some(),"output_tokens":accumulator.usage.as_ref().map(|v|v["completion_tokens"].clone())}));
    }
    let response:Value=client.inference().responses(&json!({"model":cfg["model_id"],"input":[{"role":"user","content":[{"type":"input_text","text":"Synthetic response fixture."}]}],"max_output_tokens":32,"store":true}))?.send().await?.json()?;
    validate_response(&response)?;
    checks.push(json!({"name":"real CPU Responses generation","result":"passed","terminal_status":response["status"],"output_tokens":response["usage"]["output_tokens"]}));
    let id = response["id"].as_str().ok_or("missing Responses ID")?;
    check(checks,"stored Responses content retrieval",async {
        let retrieved: Value = client.api().get_project_id_endpoint_slug_v1_responses_id(id).send().await?.json()?;
        validate_response(&retrieved)?;
        if retrieved["id"] != response["id"] || retrieved["output"][0]["content"] != response["output"][0]["content"] { return Err("stored Responses round trip".into()); }
        Ok(json!({"output_item_id_stable":retrieved["output"][0]["id"] == response["output"][0]["id"]}))
    }).await;
    check(checks, "stored Responses input-item persistence", async {
        let items: Value = client
            .api()
            .get_project_id_endpoint_slug_v1_responses_id_input_items(id)
            .send()
            .await?
            .json()?;
        if items["data"].as_array().is_none_or(Vec::is_empty) {
            return Err("Responses input items missing".into());
        }
        Ok(json!({"input_items":items["data"].as_array().unwrap().len()}))
    })
    .await;
    check(checks, "stored Responses listing", async {
        let listing: Value = client
            .api()
            .get_project_id_endpoint_slug_v1_responses()
            .query("limit", "100")
            .send()
            .await?
            .json()?;
        if !listing["data"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["id"] == response["id"]))
        {
            return Err("Responses missing from listing".into());
        }
        Ok(json!({}))
    })
    .await;
    check(checks,"Responses conversation chaining and child deletion",async {
        let chained: Value = client.inference().responses(&json!({"model":cfg["model_id"],"input":"Continue the synthetic fixture.","previous_response_id":id,"max_output_tokens":16,"store":true}))?.send().await?.json()?;
        validate_response(&chained)?;
        if chained["previous_response_id"] != response["id"] { return Err("Responses chaining contract".into()); }
        let chained_id = chained["id"].as_str().ok_or("missing chained ID")?;
        client.api().delete_project_id_endpoint_slug_v1_responses_id(chained_id).send().await?;
        Ok(json!({}))
    }).await;
    check(checks,"real CPU Responses SSE terminal snapshot and usage",async {
        let mut stream=client.inference().stream_responses(&json!({"model":cfg["model_id"],"input":"Synthetic streamed response fixture.","max_output_tokens":32,"store":false}))?.stream().await?;
        let mut count = 0;
        let mut terminal = None;
        while let Some(event) = stream.next().await {
            let event = event?;
            let value: Value = event.json()?;
            let kind = value["type"].as_str().or(event.event.as_deref()).unwrap_or_default();
            if !value["error"].is_null() || matches!(kind, "response.failed" | "response.cancelled" | "error") { return Err("response stream error".into()); }
            if matches!(kind, "response.completed" | "response.incomplete") {
                validate_response(&value["response"])?;
                terminal = Some(value["response"]["status"].clone());
            }
            count += 1;
        }
        if count == 0 || terminal.is_none() { return Err("Responses stream missing successful terminal snapshot".into()); }
        Ok(json!({"events":count,"terminal_status":terminal,"usage_verified":true}))
    }).await;
    check(checks,"background Responses queue cancel retrieve and delete",async {
        let background: Value = client.inference().responses(&json!({"model":cfg["model_id"],"input":"Synthetic cancellable background fixture.","max_output_tokens":64,"store":true,"background":true}))?.send().await?.json()?;
        let background_id = background["id"].as_str().ok_or("missing background ID")?;
        if !matches!(background["status"].as_str(), Some("queued" | "in_progress")) { return Err("background Responses not asynchronous".into()); }
        let cancelled: Value = client.api().post_project_id_endpoint_slug_v1_responses_id_cancel(background_id).send().await?.json()?;
        if cancelled["status"] != "cancelled" { return Err("background cancellation contract".into()); }
        let cancelled_get: Value = client.api().get_project_id_endpoint_slug_v1_responses_id(background_id).send().await?.json()?;
        if cancelled_get["status"] != "cancelled" { return Err("background cancellation persistence".into()); }
        let cleanup_status = match client.api().delete_project_id_endpoint_slug_v1_responses_id(background_id).send().await {
            Ok(response) => response.status,
            Err(erebine::Error::Api {status:404,..}) => 404,
            Err(error) => return Err(error.into()),
        };
        Ok(json!({"cleanup_status":cleanup_status}))
    }).await;
    check(
        checks,
        "delete stored Responses and verify absence",
        async {
            let deleted: Value = client
                .api()
                .delete_project_id_endpoint_slug_v1_responses_id(id)
                .send()
                .await?
                .json()?;
            if deleted["deleted"] != true {
                return Err("Responses deletion contract".into());
            }
            if !matches!(
                client
                    .api()
                    .get_project_id_endpoint_slug_v1_responses_id(id)
                    .send()
                    .await,
                Err(erebine::Error::Api { status: 404, .. })
            ) {
                return Err("deleted Responses remains accessible".into());
            }
            Ok(json!({}))
        },
    )
    .await;
    Ok(())
}
#[tokio::main]
async fn main() {
    let mut checks = vec![];
    let ran = match run(&mut checks).await {
        Ok(()) => true,
        Err(error) => {
            let record = match error.downcast_ref::<erebine::Error>() {
                Some(erebine::Error::Api { status, .. }) => {
                    json!({"name":"worker setup or generation","result":"http_failed","status":status})
                }
                _ => {
                    json!({"name":"worker setup or generation","result":"response_contract_failed"})
                }
            };
            checks.push(record);
            false
        }
    };
    let ok = ran && checks.iter().all(|value| value["result"] == "passed");
    println!(
        "{}",
        json!({"sdk":"rust","checks":checks,"completed":ok,"responses_only":std::env::var("EREBINE_WORKER_INFERENCE_MODE").as_deref()==Ok("responses")})
    );
    if !ok {
        eprintln!("Worker inference validation did not complete; credentials, identifiers and response bodies were not logged.");
        std::process::exit(1);
    }
}
