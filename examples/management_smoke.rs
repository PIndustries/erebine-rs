//! Opt-in disposable-resource conformance checks through a credential-injecting
//! loopback proxy. The real account key is never received or printed here.
use erebine::{json, multipart, operations, Client, Error, Surface, Value};
use std::collections::BTreeMap;
fn resolve(value: &Value, vars: &BTreeMap<String, Value>) -> Result<Value, Error> {
    match value {
        Value::String(s)
            if s.starts_with("${") && s.ends_with('}') && s.matches("${").count() == 1 =>
        {
            let name = &s[2..s.len() - 1];
            vars.get(name)
                .cloned()
                .ok_or_else(|| Error::Invalid(format!("missing variable {name}")))
        }
        Value::String(s) => {
            let mut result = s.clone();
            for (name, value) in vars {
                if let Some(value) = value.as_str() {
                    result = result.replace(&format!("${{{name}}}"), value);
                }
            }
            if result.contains("${") {
                return Err(Error::Invalid("missing interpolated variable".into()));
            }
            Ok(json!(result))
        }
        Value::Array(a) => Ok(Value::Array(
            a.iter()
                .map(|v| resolve(v, vars))
                .collect::<Result<_, _>>()?,
        )),
        Value::Object(o) => Ok(Value::Object(
            o.iter()
                .map(|(k, v)| Ok((k.clone(), resolve(v, vars)?)))
                .collect::<Result<_, Error>>()?,
        )),
        _ => Ok(value.clone()),
    }
}
fn operation(name: &str) -> Result<erebine::Operation, Error> {
    operations::ALL
        .iter()
        .find(|o| o.name == name)
        .copied()
        .ok_or_else(|| Error::Invalid("scenario operation missing".into()))
}
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let origin = std::env::var("EREBINE_MANAGEMENT_PROXY")?;
    if !origin.starts_with("http://127.0.0.1:") {
        return Err("management validation requires a loopback proxy".into());
    }
    let bootstrap = Client::new(&origin, Surface::Router)?;
    let config: Value = bootstrap
        .request("GET", "/__management/config")
        .query("client", "rust")
        .send()
        .await?
        .json()?;
    let mut vars: BTreeMap<String, Value> = config
        .as_object()
        .ok_or("proxy config missing")?
        .clone()
        .into_iter()
        .collect();
    let client = bootstrap.project(config["project_id"].as_str().ok_or("project id missing")?);
    let scenario_text = if let Ok(path) = std::env::var("EREBINE_MANAGEMENT_SCENARIOS") {
        std::fs::read_to_string(path)?
    } else {
        include_str!("../api/management-scenarios.json").to_owned()
    };
    let scenarios: Value = serde_json::from_str(&scenario_text)?;
    let mut results = vec![];
    let mut cleanup: Vec<Value> = vec![];
    let mut cleaned_paths = std::collections::BTreeSet::new();
    for original in scenarios["steps"]
        .as_array()
        .ok_or("scenario steps missing")?
    {
        let name = original["name"].as_str().unwrap_or("unnamed");
        let step = match resolve(original, &vars) {
            Ok(s) => s,
            Err(_) => {
                results.push(json!({"name":name,"result":"skipped_dependency"}));
                continue;
            }
        };
        let mut request = client.operation(operation(
            step["operation"].as_str().ok_or("operation missing")?,
        )?);
        for (name, value) in step["parameters"].as_object().ok_or("parameters missing")? {
            request = request.param(name, value.as_str().ok_or("parameter is not a string")?);
        }
        if let Some(headers) = step.get("headers").and_then(Value::as_object) {
            for (name, value) in headers {
                request = request.header(name, value.as_str().ok_or("header not a string")?)?;
            }
        }
        if let Some(pairs) = step.get("query").and_then(Value::as_array) {
            for pair in pairs {
                request = request.query(
                    pair[0].as_str().ok_or("query name missing")?,
                    pair[1].as_str().ok_or("query value missing")?,
                );
            }
        }
        if let Some(encoded) = step.get("body_base64").and_then(Value::as_str) {
            use base64::{engine::general_purpose::STANDARD, Engine};
            request = request.bytes(
                STANDARD.decode(encoded)?,
                step["content_type"]
                    .as_str()
                    .unwrap_or("application/octet-stream"),
            )?;
        }
        if let Some(body) = step.get("body") {
            request = request.json(body)?;
        }
        if let Some(form) = step.get("multipart") {
            let mut multipart = multipart::Form::new();
            for field in form["fields"].as_array().ok_or("form fields missing")? {
                multipart = multipart.text(
                    field[0].as_str().unwrap().to_owned(),
                    field[1].as_str().unwrap().to_owned(),
                );
            }
            let file = &form["file"];
            let part = multipart::Part::bytes(file["text"].as_str().unwrap().as_bytes().to_vec())
                .file_name(file["filename"].as_str().unwrap().to_owned())
                .mime_str(file["content_type"].as_str().unwrap())?;
            multipart = multipart.part(file["field"].as_str().unwrap().to_owned(), part);
            request = request.multipart(multipart);
        }
        if let Some(ms) = step["delay_ms"].as_u64() {
            tokio::time::sleep(std::time::Duration::from_millis(ms.min(5000))).await;
        }
        let sent_path = request.url()?.path().to_owned();
        match request.send().await {
            Ok(response) => {
                if operation(step["operation"].as_str().unwrap())?.method == "DELETE" {
                    cleaned_paths.insert(sent_path);
                }
                let body = response.json::<Value>().ok();
                let mut passed = step.get("expected_error").is_none()
                    && step
                        .get("expected_status")
                        .and_then(Value::as_array)
                        .map(|values| {
                            values
                                .iter()
                                .any(|s| s.as_u64() == Some(response.status.into()))
                        })
                        .unwrap_or(true);
                if let Some(spec) = step.get("save_match") {
                    let selected = body
                        .as_ref()
                        .and_then(|b| b.pointer(spec["pointer"].as_str().unwrap()))
                        .and_then(Value::as_array)
                        .and_then(|items| {
                            items.iter().find(|item| {
                                item.get(spec["field"].as_str().unwrap()) == spec.get("equals")
                            })
                        })
                        .and_then(|item| item.pointer(spec["value_pointer"].as_str().unwrap()))
                        .filter(|v| v.is_string())
                        .cloned();
                    if let Some(value) = selected {
                        vars.insert(spec["variable"].as_str().unwrap().to_owned(), value);
                    } else {
                        passed = false;
                    }
                }
                if let Some(save) = original.get("save").and_then(Value::as_object) {
                    for (key, pointers) in save {
                        let found = pointers.as_array().unwrap().iter().find_map(|p| {
                            body.as_ref()
                                .and_then(|b| b.pointer(p.as_str().unwrap()))
                                .filter(|v| v.is_string())
                                .cloned()
                        });
                        if let Some(value) = found {
                            vars.insert(key.clone(), value);
                        } else {
                            passed = false;
                        }
                    }
                    if let Some(spec) = original.get("cleanup") {
                        if let Ok(spec) = resolve(spec, &vars) {
                            cleanup.push(spec);
                        } else {
                            passed = false;
                        }
                    }
                }
                if let Some(spec) = step.get("cleanup_matching") {
                    if let Some(items) = body
                        .as_ref()
                        .and_then(|b| b.pointer(spec["pointer"].as_str().unwrap()))
                        .and_then(Value::as_array)
                    {
                        for item in items.iter().filter(|item| {
                            item[spec["field"].as_str().unwrap()]
                                .as_str()
                                .is_some_and(|v| v.starts_with(spec["prefix"].as_str().unwrap()))
                        }) {
                            if let Some(id) = item["id"].as_str() {
                                cleanup.push(json!({"method":"DELETE","path":spec["path"].as_str().unwrap().replace("{id}",id)}));
                            }
                        }
                    } else {
                        passed = false;
                    }
                }
                if let Some(assertions) = step.get("assertions").and_then(Value::as_object) {
                    for (pointer, expected) in assertions {
                        passed &= body.as_ref().and_then(|b| b.pointer(pointer)) == Some(expected);
                    }
                }
                if let Some(expected) = step.get("expected_text").and_then(Value::as_str) {
                    passed &= response.text().ok() == Some(expected);
                }
                if let Some(lengths) = step.get("min_array_lengths").and_then(Value::as_object) {
                    for (pointer, min) in lengths {
                        passed &= body
                            .as_ref()
                            .and_then(|b| b.pointer(pointer))
                            .and_then(Value::as_array)
                            .is_some_and(|a| a.len() >= min.as_u64().unwrap() as usize);
                    }
                }
                results.push(json!({"name":name,"status":response.status,"result":if passed {"passed"} else {"response_contract_failed"},"json_fields":body.as_ref().and_then(Value::as_object).map(|o|o.keys().cloned().collect::<Vec<_>>())}));
            }
            Err(Error::Api { status, body, .. }) => {
                let code = serde_json::from_slice::<Value>(&body)
                    .ok()
                    .and_then(|b| b["error"]["code"].as_str().map(str::to_owned));
                let expected = original.get("expected_error").is_some_and(|e| {
                    e["status"].as_u64() == Some(status.into())
                        && e.get("codes")
                            .and_then(Value::as_array)
                            .map(|codes| codes.iter().any(|c| c.as_str() == code.as_deref()))
                            .unwrap_or(true)
                });
                results.push(json!({"name":name,"status":status,"result":if expected {"expected_rejection"} else if code.as_deref()==Some("no_active_endpoint") {"environment_unavailable"} else {"http_failed"},"code":code}));
            }
            Err(_) => results.push(json!({"name":name,"result":"transport_failed"})),
        }
    }
    let mut cleanup_results = vec![];
    for spec in cleanup.iter().rev() {
        let path = spec["path"].as_str().unwrap();
        let mut path = path.to_owned();
        for (name, value) in &vars {
            if let Some(value) = value.as_str() {
                path = path.replace(&format!("{{{name}}}"), value);
            }
        }
        if !cleaned_paths.insert(path.clone()) {
            continue;
        }
        let mut request = client.request(spec["method"].as_str().unwrap(), &path);
        if let Some(headers) = spec.get("headers").and_then(Value::as_object) {
            for (name, value) in headers {
                request = request.header(name, value.as_str().unwrap())?;
            }
        }
        match request.send().await {
            Ok(response) => {
                cleanup_results.push(json!({"status":response.status,"result":"passed"}))
            }
            Err(Error::Api { status: 404, .. }) => {
                cleanup_results.push(json!({"status":404,"result":"already_absent"}))
            }
            Err(Error::Api { status, .. }) => {
                cleanup_results.push(json!({"status":status,"result":"failed"}))
            }
            Err(_) => cleanup_results.push(json!({"result":"transport_failed"})),
        }
    }
    let failed = results.iter().any(|r| {
        matches!(
            r["result"].as_str(),
            Some("http_failed" | "transport_failed" | "response_contract_failed")
        )
    }) || cleanup_results
        .iter()
        .any(|r| matches!(r["result"].as_str(), Some("failed" | "transport_failed")));
    println!(
        "{}",
        serde_json::to_string(&json!({"sdk":"rust","steps":results,"cleanup":cleanup_results}))?
    );
    if failed {
        return Err("live validation has failed checks or cleanup".into());
    }
    Ok(())
}
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!("Management validation did not complete; credentials and project values were not logged.");
        std::process::exit(1);
    }
}
