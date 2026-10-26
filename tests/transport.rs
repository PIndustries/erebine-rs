use erebine::{json, multipart, operations, Client, Error, Routing, Surface, Value};
use futures_util::StreamExt;
use std::{
    path::PathBuf,
    process::{Child, Command},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};
static FIXTURE_COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    process: Child,
    file: PathBuf,
    origin: String,
}
impl Fixture {
    fn start() -> Self {
        let file = std::env::temp_dir().join(format!(
            "erebine-fixture-{}-{}-{}.port",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let process = Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/http_fixture.py"
            ))
            .arg("--port-file")
            .arg(&file)
            .spawn()
            .unwrap();
        let start = Instant::now();
        let port = loop {
            if let Ok(s) = std::fs::read_to_string(&file) {
                if let Ok(port) = s.parse::<u16>() {
                    break port;
                }
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "fixture failed to start"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        Self {
            process,
            file,
            origin: format!("http://127.0.0.1:{port}"),
        }
    }
    fn client(&self) -> Client {
        Client::new(&self.origin, Surface::Router)
            .unwrap()
            .bearer("fake-fixture-key")
            .unwrap()
            .project("proj_test")
            .routing(Routing::Endpoint("test".into()))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = std::fs::remove_file(&self.file);
    }
}
#[tokio::test]
async fn json_headers_query_and_metadata() {
    let f = Fixture::start();
    let c = f.client();
    let r = c
        .request("POST", "/echo/{id}")
        .param("id", "a/b?#% 🌎")
        .query("tag", "a+b")
        .query("tag", "two")
        .json(&json!({"x_vendor":{"enabled":true},"model":"test"}))
        .unwrap()
        .header("x-conversation-id", "stable")
        .unwrap()
        .send()
        .await
        .unwrap();
    assert_eq!(r.request_id(), Some("fixture-request"));
    let v: Value = r.json().unwrap();
    assert_eq!(v["method"], "POST");
    assert!(v["path"]
        .as_str()
        .unwrap()
        .starts_with("/echo/a%2Fb%3F%23%25%20%F0%9F%8C%8E?tag=a%2Bb&tag=two"));
    let body: Value = serde_json::from_str(v["body"].as_str().unwrap()).unwrap();
    assert_eq!(body["x_vendor"]["enabled"], true);
}
#[tokio::test]
async fn errors_empty_binary_and_redirects() {
    let f = Fixture::start();
    let c = f.client();
    let err = c.request("GET", "/error").send().await.unwrap_err();
    assert_eq!(
        err.api_body().unwrap()["error"]["type"],
        "future_vendor_error"
    );
    assert!(matches!(err,Error::Api{status:429,retry_after:Some(ref s),..} if s=="3"));
    assert!(c
        .request("DELETE", "/empty")
        .send()
        .await
        .unwrap()
        .body
        .is_empty());
    assert_eq!(
        c.request("GET", "/binary").send().await.unwrap().body,
        vec![0, 255, 10, 128]
    );
    assert!(matches!(
        c.request("GET", "/redirect").send().await,
        Err(Error::Api { status: 302, .. })
    ));
}
#[tokio::test]
async fn multipart_and_octet_stream() {
    let f = Fixture::start();
    let c = f.client();
    let form = multipart::Form::new().text("purpose", "batch").part(
        "file",
        multipart::Part::bytes(b"abc\n".to_vec()).file_name("batch.jsonl"),
    );
    let r = c
        .request("POST", "/echo")
        .multipart(form)
        .send()
        .await
        .unwrap();
    let v: Value = r.json().unwrap();
    assert!(v["body"]
        .as_str()
        .unwrap()
        .contains("filename=\"batch.jsonl\""));
    let v: Value = c
        .request("POST", "/echo")
        .bytes(b"raw".to_vec(), "application/octet-stream")
        .unwrap()
        .header("x-chunk-checksum", "fake-sha256")
        .unwrap()
        .send()
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(v["body"], "raw");
}
#[tokio::test]
async fn streaming_preserves_vendor_events_and_stops_at_done() {
    let f = Fixture::start();
    let c = f.client();
    let mut stream = c.request("GET", "/stream").stream().await.unwrap();
    assert_eq!(stream.headers["x-request-id"], "fixture-stream");
    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.event.as_deref(), Some("x_reasoning"));
    assert_eq!(first.json().unwrap()["text"], "héllo 🌎");
    assert_eq!(
        stream.next().await.unwrap().unwrap().event.as_deref(),
        Some("response.completed")
    );
    assert!(stream.next().await.is_none());
}
#[test]
fn all_catalog_operations_resolve_and_paths_are_safe() {
    let router = Client::new("https://api.erebine.ai", Surface::Router).unwrap();
    let dashboard = Client::new("https://erebine.ai", Surface::Dashboard).unwrap();
    for op in operations::ALL {
        let client = if op.surface == Surface::Router {
            &router
        } else {
            &dashboard
        };
        let mut r = client.operation(*op);
        for segment in op.path.split('/') {
            if segment.starts_with('{') {
                r = r.param(&segment[1..segment.len() - 1], "test");
            }
        }
        assert!(r.url().is_ok(), "{}", op.name);
    }
    assert!(router.request("GET", "//evil.example/path").url().is_err());
    assert!(router
        .request("GET", "/a/{id}")
        .param("id", "..")
        .url()
        .is_err());
    assert!(router
        .request("GET", "/a/{relative_path}")
        .param("relative_path", "../secret")
        .url()
        .is_err());
    assert!(router
        .operation(operations::CHAT_COMPLETIONS_CREATE)
        .url()
        .is_err());
    assert!(router
        .operation(
            *operations::ALL
                .iter()
                .find(|x| x.surface == Surface::Dashboard)
                .unwrap()
        )
        .url()
        .is_err());
    assert!(!format!("{:?}", router.bearer("secret-value").unwrap()).contains("secret-value"));
}
#[test]
fn direct_semantic_and_custom_domain_routing() {
    let c = Client::router(
        "fake",
        "proj",
        Routing::Direct {
            workspace: Some("ws".into()),
        },
    )
    .unwrap();
    assert_eq!(
        c.operation(operations::CHAT_COMPLETIONS_CREATE)
            .url()
            .unwrap()
            .path(),
        "/proj/ws/_direct/v1/chat/completions"
    );
    let c = c.routing(Routing::Semantic {
        workspace: "ws".into(),
    });
    assert_eq!(
        c.operation(operations::RESPONSES_CREATE)
            .url()
            .unwrap()
            .path(),
        "/proj/ws/_semantic/router/v1/responses"
    );
    let c = c.custom_domain();
    assert_eq!(
        c.operation(operations::MODELS_LIST).url().unwrap().path(),
        "/ws/_semantic/router/v1/models"
    );
}

#[tokio::test]
async fn mcp_session_negotiation_json_sse_calls_errors_and_close() {
    let f = Fixture::start();
    let c = f.client();
    let mut session = erebine::mcp::McpSession::connect(c).await.unwrap();
    assert_eq!(session.initialization["protocolVersion"], "2025-06-18");
    let tools = session.list_tools(None).await.unwrap();
    assert_eq!(tools["tools"][0]["name"], "fixture.echo");
    let result = session
        .call_tool("fixture.echo", json!({"text":"hello"}))
        .await
        .unwrap();
    assert_eq!(result["structuredContent"]["text"], "hello");
    assert!(
        matches!(session.call("unknown",json!({})).await,Err(Error::Rpc(ref e)) if e["code"]==-32601 && e["x_vendor"]==true)
    );
    session.close().await.unwrap();
    assert!(session.list_tools(None).await.is_err());
}

#[tokio::test]
async fn inference_helpers_preserve_wire_fields_and_audio_formats() {
    use erebine::types::*;
    let f = Fixture::start();
    let client = f.client().header("x-fixture-echo", "1").unwrap();
    let inference = client.inference();
    let mut chat = ChatCompletionRequest::new("fixture-model", vec![Message::user("hi")]);
    chat.extra
        .insert("model".into(), json!("must-not-override"));
    chat.extra.insert("response_format".into(),json!({"type":"json_schema","json_schema":{"name":"answer","strict":true,"schema":{"type":"object"}}}));
    chat.extra.insert(
        "tools".into(),
        json!([{"type":"function","function":{"name":"echo","parameters":{"type":"object"}}}]),
    );
    chat.extra.insert("x_future".into(), json!({"on":true}));
    let cases = vec![
        (inference.chat(&chat).unwrap(), "/v1/chat/completions"),
        (
            inference
                .responses(&ResponsesRequest::new(
                    "fixture-model",
                    json!([{"type":"function_call_output","call_id":"call_test","output":"ok"}]),
                ))
                .unwrap(),
            "/v1/responses",
        ),
        (
            inference
                .embeddings(&EmbeddingRequest::new(
                    "fixture-model",
                    json!([[1, 2], [3, 4]]),
                ))
                .unwrap(),
            "/v1/embeddings",
        ),
        (
            inference
                .rerank(&RerankRequest::new(
                    "fixture-model",
                    "question",
                    vec![json!("doc"), json!({"text":"other"})],
                ))
                .unwrap(),
            "/v1/rerank",
        ),
        (
            inference
                .score(&ScoreRequest::new(
                    "fixture-model",
                    "question",
                    vec!["doc".into()],
                ))
                .unwrap(),
            "/v1/score",
        ),
        (
            inference
                .speech(&SpeechRequest::new("fixture-model", "hello", "alloy"))
                .unwrap(),
            "/v1/audio/speech",
        ),
        (
            inference
                .system_one(&SystemOneRequest::new(
                    "fixture-model",
                    json!({"state":"test"}),
                    Default::default(),
                ))
                .unwrap(),
            "/v1/systemone",
        ),
    ];
    for (request, suffix) in cases {
        let value: Value = request.send().await.unwrap().json().unwrap();
        assert!(value["path"].as_str().unwrap().ends_with(suffix));
        let body: Value = serde_json::from_str(value["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["model"], "fixture-model");
        if suffix == "/v1/chat/completions" {
            assert_eq!(body["x_future"]["on"], true);
            assert_eq!(body["tools"][0]["function"]["name"], "echo");
            assert_eq!(body["response_format"]["type"], "json_schema");
        }
    }
    let file = AudioFile::new("clip.wav", "audio/wav", b"RIFFfixture".to_vec());
    let mut body = AudioTranscriptionRequest::new("fixture-model", file);
    body.fields = vec![
        ("response_format".into(), "text".into()),
        ("timestamp_granularities[]".into(), "word".into()),
        ("timestamp_granularities[]".into(), "segment".into()),
    ];
    for request in [
        inference.transcribe(&body).unwrap(),
        inference.translate(&body).unwrap(),
    ] {
        let value: Value = request.send().await.unwrap().json().unwrap();
        let wire = value["body"].as_str().unwrap();
        assert!(wire.contains("filename=\"clip.wav\""));
        assert!(wire.contains("RIFFfixture"));
        assert!(wire.contains("timestamp_granularities[]"));
    }
    body.fields.push(("model".into(), "bad".into()));
    assert!(inference.transcribe(&body).is_err());
    let model: Value = inference
        .model("org/model")
        .send()
        .await
        .unwrap()
        .json()
        .unwrap();
    assert!(model["path"]
        .as_str()
        .unwrap()
        .ends_with("/v1/models/org%2Fmodel"));
    let audio = f
        .client()
        .inference()
        .speech(&SpeechRequest::new("fixture-model", "hello", "alloy"))
        .unwrap()
        .send()
        .await
        .unwrap();
    assert_eq!(audio.body, b"RIFFfixtureWAVE");
    assert_eq!(audio.headers["content-type"], "audio/wav");
}
#[tokio::test]
async fn inference_streaming_assembles_tool_calls_reasoning_and_usage() {
    let f = Fixture::start();
    let client = f.client();
    let mut stream=client.inference().stream_chat(&json!({"model":"fixture","messages":[],"stream":false,"stream_options":{"include_usage":true},"x_future":1})).unwrap().stream().await.unwrap();
    let mut accumulator = erebine::accumulator::ChatAccumulator::default();
    while let Some(event) = stream.next().await {
        accumulator.push(&event.unwrap()).unwrap();
    }
    assert_eq!(accumulator.raw[0]["fixture_request"]["stream"], true);
    assert_eq!(accumulator.raw[0]["fixture_request"]["x_future"], 1);
    assert_eq!(accumulator.choices[&0].reasoning, "thinking");
    assert_eq!(accumulator.choices[&0].content, "");
    assert_eq!(
        accumulator.choices[&0].tool_calls[&0].arguments,
        "{\"a\":1}"
    );
    assert_eq!(
        accumulator.choices[&0].finish_reason.as_deref(),
        Some("tool_calls")
    );
    assert_eq!(accumulator.usage.as_ref().unwrap()["total_tokens"], 5);
    for (kind, request) in [
        (
            "responses",
            client
                .inference()
                .stream_responses(&json!({"model":"fixture","input":"hi"}))
                .unwrap(),
        ),
        (
            "speech",
            client
                .inference()
                .stream_speech(&json!({"model":"fixture","input":"hi","voice":"alloy"}))
                .unwrap(),
        ),
    ] {
        let mut stream = request.stream().await.unwrap();
        let first = stream.next().await.unwrap().unwrap().json().unwrap();
        if kind == "speech" {
            assert_eq!(first["fixture_request"]["stream_format"], "sse");
        } else {
            assert_eq!(first["fixture_request"]["stream"], true);
        }
    }
}
#[test]
fn stream_interleaving_and_tool_round_trip_helpers() {
    let mut a = erebine::accumulator::ChatAccumulator::default();
    for value in [
        json!({"id":"chat_original","model":"model_original","choices":[{"index":1,"delta":{"content":"B","refusal":"refused","reasoning_content":null,"reasoning":"R"}},{"index":0,"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"sum","arguments":"{\"b\":"}},{"index":0,"id":"a","function":{"name":"echo","arguments":"{\"a\":"}}]}}]}),
        json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}},{"index":1,"function":{"arguments":"2}"}}]}},{"index":1,"delta":{"content":"2"},"finish_reason":"future_reason"}]}),
        json!({"type":"x_future","id":"vendor_event","model":"vendor_model","usage":{"total_tokens":999},"vendor":{"kept":true},"error":null}),
    ] {
        a.push_json(value).unwrap();
    }
    assert_eq!(a.choices[&1].content, "B2");
    assert_eq!(a.choices[&1].refusal, "refused");
    assert_eq!(a.choices[&1].reasoning, "R");
    assert_eq!(a.choices[&0].tool_calls[&1].arguments, "{\"b\":2}");
    assert_eq!(a.raw.len(), 3);
    assert_eq!(a.id.as_deref(), Some("chat_original"));
    assert_eq!(a.model.as_deref(), Some("model_original"));
    assert!(a.usage.is_none());
    assert!(a
        .push_json(json!({"error":{"code":"future_error"}}))
        .is_err());
    assert_eq!(a.raw.len(), 4);
    let message = erebine::types::Message::tool("call_test", "ok");
    let value = serde_json::to_value(message).unwrap();
    assert_eq!(value["tool_call_id"], "call_test");
    assert_eq!(
        erebine::types::output_text(
            &json!({"output":[{"type":"reasoning","text":"hidden"},{"type":"message","content":[{"type":"output_text","text":"hi"}]}]})
        ),
        "hi"
    );
}
#[cfg(feature = "realtime")]
#[tokio::test]
async fn native_realtime_transcription_websocket() {
    let f = Fixture::start();
    let client = f.client();
    let mut session = client.realtime().await.unwrap();
    assert_eq!(
        session.headers["x-fixture-authorization"],
        "Bearer fake-fixture-key"
    );
    session
        .configure_transcription("fixture-model", "pcm16")
        .await
        .unwrap();
    assert_eq!(
        session.receive_json().await.unwrap().unwrap()["type"],
        "session.updated"
    );
    session.append_audio(&[0, 255, 128, 1]).await.unwrap();
    assert_eq!(session.receive_json().await.unwrap().unwrap()["bytes"], 4);
    session.commit_audio().await.unwrap();
    assert_eq!(
        session.receive_json().await.unwrap().unwrap()["delta"],
        "héllo "
    );
    assert_eq!(
        session.receive_json().await.unwrap().unwrap()["transcript"],
        "héllo 🌎"
    );
    session
        .send_json(&json!({"type":"x_future","vendor":true}))
        .await
        .unwrap();
    assert_eq!(
        session.receive_json().await.unwrap().unwrap()["payload"]["vendor"],
        true
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn official_cli_contracts_replay_through_native_transport() {
    let evidence: Value = serde_json::from_str(include_str!("../api/cli-contracts.json")).unwrap();
    let fixtures = evidence["requests"].as_array().unwrap();
    assert_eq!(fixtures.len(), 283);
    let f = Fixture::start();
    let client = f.client().header("x-fixture-echo", "1").unwrap();
    for fixture in fixtures {
        let name = fixture["operation"].as_str().unwrap();
        let operation = *operations::ALL
            .iter()
            .find(|o| o.name == name)
            .expect("fixture operation missing");
        let mut request = client.operation(operation);
        for (name, value) in fixture["parameters"].as_object().unwrap() {
            request = request.param(name, value.as_str().unwrap());
        }
        for pair in fixture["query"].as_array().unwrap() {
            request = request.query(pair[0].as_str().unwrap(), pair[1].as_str().unwrap());
        }
        if let Some(headers) = fixture["headers"].as_object() {
            for (name, value) in headers {
                request = request.header(name, value.as_str().unwrap()).unwrap();
            }
        }
        if let Some(ct) = fixture["content_type"].as_str() {
            request = request.header("content-type", ct).unwrap();
        }
        // All captured bodies are UTF-8, including synthetic multipart files.
        let body = if !fixture["body_base64"].as_str().unwrap().is_empty()
            && fixture["content_type"]
                .as_str()
                .is_some_and(|ct| ct.contains("application/json"))
        {
            // Exercise arbitrary JSON field handling without relying on key order.
            request = request.json(&fixture["body_json"]).unwrap();
            None
        } else if fixture["body_base64"].as_str().unwrap().is_empty() {
            None
        } else {
            use base64::{engine::general_purpose::STANDARD, Engine};
            let bytes = STANDARD
                .decode(fixture["body_base64"].as_str().unwrap())
                .unwrap();
            request = request
                .bytes(bytes.clone(), fixture["content_type"].as_str().unwrap())
                .unwrap();
            Some(bytes)
        };
        let expected_url = request.url().unwrap();
        assert_eq!(
            expected_url.path(),
            fixture["expected_path"].as_str().unwrap(),
            "{}",
            fixture["command"]
        );
        let response: Value = request.send().await.unwrap().json().unwrap();
        assert_eq!(response["method"], operation.method);
        if let Some(headers) = fixture["headers"].as_object() {
            for (name, value) in headers {
                let found = response["headers"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(name))
                    .unwrap()
                    .1;
                assert_eq!(found, value);
            }
        }
        if fixture["body_base64"] == "" {
            assert_eq!(response["body"], "");
        }

        assert_eq!(
            response["path"].as_str().unwrap(),
            format!(
                "{}{}",
                expected_url.path(),
                expected_url
                    .query()
                    .map(|q| format!("?{q}"))
                    .unwrap_or_default()
            )
        );
        if !fixture["body_json"].is_null() {
            assert_eq!(
                serde_json::from_str::<Value>(response["body"].as_str().unwrap()).unwrap(),
                fixture["body_json"]
            );
        }
        if let Some(body) = body {
            assert_eq!(response["body"].as_str().unwrap().as_bytes(), body);
        }
    }
}

#[tokio::test]
async fn every_catalog_binding_crosses_native_transport() {
    let f = Fixture::start();
    for op in operations::ALL {
        let client = Client::new(&f.origin, op.surface)
            .unwrap()
            .project("proj_fixture")
            .routing(Routing::Endpoint("endpoint_fixture".into()))
            .header("x-fixture-echo", "1")
            .unwrap();
        let mut request = client
            .operation(*op)
            .query("tag", "first + value")
            .query("tag", "second");
        for name in op
            .path
            .split('{')
            .skip(1)
            .map(|p| p.split('}').next().unwrap())
        {
            request = request.param(name, format!("fixture_{name}"));
        }
        let expected = request.url().unwrap();
        let response = request
            .send()
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", op.name));
        let echo: Value = response.json().unwrap();
        assert_eq!(echo["method"], op.method, "{}", op.name);
        assert_eq!(
            echo["path"],
            format!("{}?{}", expected.path(), expected.query().unwrap()),
            "{}",
            op.name
        );
    }
}
#[tokio::test]
async fn native_api_methods_and_workspace_endpoint_routing() {
    let f = Fixture::start();
    let client = f
        .client()
        .routing(Routing::WorkspaceEndpoint {
            workspace: "ws fixture".into(),
            endpoint: "endpoint/fixture".into(),
        })
        .header("x-fixture-echo", "1")
        .unwrap();
    let echo: Value = client
        .api()
        .post_project_id_endpoint_slug_v1_chat_completions()
        .json(&json!({"model":"fixture", "messages":[], "x_future":true}))
        .unwrap()
        .send()
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        echo["path"],
        "/proj_test/ws%20fixture/endpoint%2Ffixture/v1/chat/completions"
    );
    let echo: Value = client
        .api()
        .patch_project_id_v1_management_endpoints_id("endpoint fixture")
        .json(&json!({"name":"fixture"}))
        .unwrap()
        .send()
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(
        echo["path"],
        "/proj_test/v1/management/endpoints/endpoint%20fixture"
    );
    assert_eq!(echo["method"], "PATCH");
    let echo: Value = client
        .api()
        .options_project_id_endpoint_slug_v1_chat_completions()
        .header("origin", "https://example.invalid")
        .unwrap()
        .header("access-control-request-method", "POST")
        .unwrap()
        .send()
        .await
        .unwrap()
        .json()
        .unwrap();
    assert_eq!(echo["method"], "OPTIONS");
    let wrong = Client::new(&f.origin, Surface::Dashboard)
        .unwrap()
        .api()
        .get_project_id_v1_models()
        .url();
    assert!(wrong.is_err());
}
#[tokio::test]
async fn mcp_prompts_resources_pagination_notifications_and_resume() {
    let f = Fixture::start();
    let mut session = erebine::mcp::McpSession::connect(f.client()).await.unwrap();
    assert_eq!(session.ping().await.unwrap(), json!({}));
    let first = session.list_prompts(None).await.unwrap();
    assert_eq!(first["nextCursor"], "fixture-next");
    let next = session
        .list_prompts(first["nextCursor"].as_str())
        .await
        .unwrap();
    assert!(next.get("nextCursor").is_none());
    assert_eq!(next["fixture_params"]["cursor"], "fixture-next");
    let prompt = session
        .get_prompt("fixture", [("topic".into(), "unicode 🌎".into())].into())
        .await
        .unwrap();
    assert_eq!(
        prompt["fixture_params"],
        json!({"name":"fixture", "arguments":{"topic":"unicode 🌎"}})
    );
    let resources = session.list_resources(Some("page2")).await.unwrap();
    assert_eq!(resources["fixture_params"]["cursor"], "page2");
    let templates = session.list_resource_templates(None).await.unwrap();
    assert_eq!(templates["resourceTemplates"][0]["x_vendor"], true);
    let uri = "erebine://fixture/resource";
    assert_eq!(
        session.read_resource(uri).await.unwrap()["contents"][0]["text"],
        "héllo 🌎"
    );
    assert_eq!(
        session.subscribe_resource(uri).await.unwrap()["fixture_params"]["uri"],
        uri
    );
    let mut events = session.events(Some("fixture-event-1")).await.unwrap();
    let event = events.next().await.unwrap().unwrap();
    assert_eq!(event.id.as_deref(), Some("fixture-event-2"));
    let message: Value = event.json().unwrap();
    assert_eq!(message["method"], "notifications/resources/updated");
    assert_eq!(message["x_last_event_id"], "fixture-event-1");
    assert!(events.next().await.is_none());
    session.unsubscribe_resource(uri).await.unwrap();
    session.close().await.unwrap();
    assert!(session.events(None).await.is_err());
    let refused = erebine::mcp::McpSession::connect(
        f.client().header("x-fixture-mcp-refuse-get", "1").unwrap(),
    )
    .await
    .unwrap();
    assert!(matches!(
        refused.events(None).await,
        Err(Error::Api { status: 405, .. })
    ));
}
