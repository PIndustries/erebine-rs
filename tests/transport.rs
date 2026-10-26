use erebine::{json, multipart, operations, Client, Error, Routing, Surface, Value};
use futures_util::StreamExt;
use std::{
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};
struct Fixture {
    process: Child,
    file: PathBuf,
    origin: String,
}
impl Fixture {
    fn start() -> Self {
        let file = std::env::temp_dir().join(format!(
            "erebine-fixture-{}-{}.port",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
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
