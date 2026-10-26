# erebine-rs

Native, asynchronous Rust SDK for Erebine, maintained by P.Industries.
Community project; not an official Erebine SDK. MIT licensed.

**Experimental 0.1:** 268 documented HTTP operation bindings, shared with
[erebine-swift](https://github.com/PIndustries/erebine-swift). Full API/schema parity
is the target; public documentation gaps and live verification remain. Read
[COVERAGE.md](COVERAGE.md) before depending on an operation in production.

## Install

Until a crates.io release is published:

```toml
[dependencies]
erebine-rs = { git = "https://github.com/PIndustries/erebine-rs" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

The crate imports as `erebine`. HTTPS uses rustls; no Python/Node runtime or
Erebine CLI is needed by the SDK. Python 3 is used only by the test fixture.

## Chat

```rust,no_run
use erebine::{Client, Routing, operations, types::{ChatCompletionRequest, Message}};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::router(
    &std::env::var("EREBINE_API_KEY")?,
    &std::env::var("EREBINE_PROJECT_ID")?,
    Routing::Direct { workspace: Some(std::env::var("EREBINE_WORKSPACE_ID")?) },
)?;
let request = ChatCompletionRequest::new(
    std::env::var("EREBINE_MODEL")?,
    vec![Message::user("Hello")],
);
let response = client.operation(operations::CHAT_COMPLETIONS_CREATE)
    .json(&request)?.send().await?;
println!("{}", response.text()?);
# Ok(()) }
```

Use `MODELS_LIST` on the same routing base to discover valid model IDs. Endpoint
slugs, direct-model IDs, and semantic routing are different addressing modes.
`Routing::Endpoint(slug)` binds to one endpoint;
`Routing::Semantic { workspace }` selects the semantic router. Set
`X-Conversation-Id` for multi-turn affinity. Custom origins use `Client::new`;
`.custom_domain()` omits the project segment when the hostname supplies it.

## Full wire access

Every catalog entry has a named `operations::...` constant. Find the exact route
and constant in [api/README.md](api/README.md). Methods, paths, and arbitrary body
fields remain visible; no application prompts or agent loops run implicitly.

```rust,no_run
use erebine::{Client, operations, json, multipart, Value};
# async fn example(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let response = client.operation(operations::POST_PROJECT_ID_V1_CONVERSATIONS)
    .json(&json!({"metadata": {"app": "example"}}))?.send().await?;
let conversation: Value = response.json()?;
let _items = client.operation(operations::GET_PROJECT_ID_V1_CONVERSATIONS_ID_ITEMS)
    .param("id", conversation["id"].as_str().unwrap())
    .query("limit", 20).send().await?;

let form = multipart::Form::new().text("purpose", "batch")
    .part("file", multipart::Part::bytes(b"{}\n".to_vec()).file_name("input.jsonl"));
let _file = client.operation(operations::FILES_CREATE).multipart(form).send().await?;
# Ok(()) }
```

Use `.bytes(data, content_type)` for upload chunks or exact raw bodies, including
checksum headers. Responses retain original bytes for downloads/audio and empty
204 responses; decode JSON explicitly when present. Query parameters can repeat.
Unknown vendor error types and fields are retained by `Error::api_body()`.
`Error::Api` includes status, headers, body, request ID, and Retry-After.
Automatic retries are disabled; a failed generation or mutation may already have
been accepted by the server.

## Streaming

Set `stream: true` in the request, call `.stream().await?`, and consume with
`futures_util::StreamExt`. Events retain `event`, `data`, `id`, and `retry`; the
stream stops at `[DONE]`. Set `X-Erebine-Vendor-Events: on` to receive vendor events.
See [examples/stream.rs](examples/stream.rs). Dropping the stream releases the
HTTP response; submit the documented cancellation operation to stop server work.
The client timeout defaults to 300 seconds and can be changed with `.timeout()`.

## Dashboard routes

Use a separate `Client::new("https://erebine.ai", Surface::Dashboard)` and supply
the required Cookie and X-CSRF-Token headers explicitly. Some dashboard routes use
bearer authentication instead; consult the linked route documentation. Router
keys do not automatically create browser sessions. Cross-surface operation/client
pairs are rejected. Redirect responses are returned as errors without following
the Location header.

## Development

```sh
python3 scripts/generate.py
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo doc --no-deps
```

Run live examples only after configuring the required environment variables;
they contact Erebine and may incur inference charges. Tests use loopback fixtures.
See [contributing](CONTRIBUTING.md), [security](SECURITY.md), and [NOTICE](NOTICE).
