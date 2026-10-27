//! Opt-in MCP live checks through a credential-injecting loopback proxy.
use erebine::{json, mcp::McpSession, operations, Client, Error, Surface, Value};
use futures_util::StreamExt;
fn record(results: &mut Vec<Value>, name: &str, result: Result<Value, Error>) -> Option<Value> {
    match result {
        Ok(value) => {
            results.push(json!({"name":name,"result":"passed"}));
            Some(value)
        }
        Err(Error::Api { status, .. }) => {
            results.push(json!({"name":name,"result":"http_rejected","status":status}));
            None
        }
        Err(Error::Rpc(value)) => {
            results.push(json!({"name":name,"result":"rpc_rejected","code":value.get("code")}));
            None
        }
        Err(_) => {
            results.push(json!({"name":name,"result":"protocol_or_transport_failed"}));
            None
        }
    }
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
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
    let client = bootstrap.project(cfg["project_id"].as_str().ok_or("project missing")?);
    let workspace: Value = client
        .api()
        .post_project_id_v1_workspaces()
        .json(&json!({"name":cfg["prefix"],"description":"Disposable MCP protocol validation"}))?
        .send()
        .await?
        .json()?;
    let id = workspace["id"].as_str().ok_or("workspace missing")?;
    let mcp_client = client.clone().header("x-erebine-workspace", id)?;
    let mut results = vec![];
    match McpSession::connect(mcp_client).await {
        Ok(mut session) => {
            results
                .push(json!({"name":"initialize and initialized notification","result":"passed"}));
            record(&mut results, "ping", session.ping().await);
            record(&mut results, "tools list", session.list_tools(None).await);
            let prompts = record(
                &mut results,
                "prompts list",
                session.list_prompts(None).await,
            );
            let resources = record(
                &mut results,
                "resources list",
                session.list_resources(None).await,
            );
            record(
                &mut results,
                "resource templates",
                session.list_resource_templates(None).await,
            );
            if let Some(prompt) = prompts
                .as_ref()
                .and_then(|p| p["prompts"].as_array())
                .and_then(|p| p.first())
            {
                let arguments = prompt["arguments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|a| a["name"].as_str())
                    .map(|name| {
                        (
                            name.to_owned(),
                            if name.contains("workspace") {
                                id.to_owned()
                            } else {
                                "Synthetic SDK validation".into()
                            },
                        )
                    })
                    .collect();
                if let Some(name) = prompt["name"].as_str() {
                    record(
                        &mut results,
                        "prompt retrieval",
                        session.get_prompt(name, arguments).await,
                    );
                }
            }
            if let Some(uri) = resources
                .as_ref()
                .and_then(|p| p["resources"].as_array())
                .and_then(|p| p.first())
                .and_then(|p| p["uri"].as_str())
            {
                record(
                    &mut results,
                    "resource read",
                    session.read_resource(uri).await,
                );
                if session.initialization["capabilities"]["resources"]["subscribe"] == true {
                    record(
                        &mut results,
                        "resource subscribe",
                        session.subscribe_resource(uri).await,
                    );
                    record(
                        &mut results,
                        "resource unsubscribe",
                        session.unsubscribe_resource(uri).await,
                    );
                } else {
                    results.push(json!({"name":"resource subscriptions","result":"capability_not_advertised"}));
                }
            }
            match tokio::time::timeout(std::time::Duration::from_secs(5), session.events(None))
                .await
            {
                Ok(Ok(mut stream)) => {
                    results.push(json!({"name":"GET event stream","result":"opened"}));
                    let event =
                        tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
                            .await;
                    results.push(json!({"name":"server notification","result":if matches!(event,Ok(Some(Ok(_)))) {"received"} else {"no_event_observed"}}));
                }
                Ok(Err(Error::Api { status, .. })) => results.push(
                    json!({"name":"GET event stream","result":"http_rejected","status":status}),
                ),
                _ => results.push(json!({"name":"GET event stream","result":"not_established"})),
            }
            match session.close().await {
                Ok(()) => results.push(json!({"name":"session close","result":"passed"})),
                Err(_) => results.push(json!({"name":"session close","result":"failed"})),
            };
        }
        Err(error) => {
            record(&mut results, "initialize", Err(error));
        }
    }
    let cleanup = client
        .operation(operations::DELETE_PROJECT_ID_V1_WORKSPACES_ID)
        .param("id", id)
        .send()
        .await;
    let cleanup_ok = cleanup.is_ok();
    println!(
        "{}",
        json!({"sdk":"rust","checks":results,"workspace_cleanup":cleanup_ok})
    );
    let failed = results.iter().any(|r| {
        matches!(
            r["result"].as_str(),
            Some("failed" | "rpc_rejected" | "protocol_or_transport_failed")
        ) || (r["result"] == "http_rejected"
            && !(r["name"] == "GET event stream" && r["status"] == 405))
    });
    if failed {
        return Err("MCP checks failed".into());
    }
    if !cleanup_ok {
        return Err("workspace cleanup failed".into());
    }
    Ok(())
}
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!(
            "MCP validation did not complete; credentials and response bodies were not logged."
        );
        std::process::exit(1)
    }
}
