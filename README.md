# erebine-rs

Native, asynchronous Rust SDK for Erebine, maintained by P.Industries.
Community project; not an official Erebine SDK. MIT licensed.

**Experimental 0.1:** 376 documentation/CLI-derived HTTP operation bindings, shared with
[erebine-swift](https://github.com/PIndustries/erebine-swift). Full API/schema parity
is the target; public documentation gaps and live verification remain. Read
[COVERAGE.md](COVERAGE.md) before depending on an operation in production.


**Inference-first SDK:** named inference APIs, typed request helpers, tool-result
messages, structured-output passthrough, stream assembly, audio, and native
realtime transcription are available. Start with [INFERENCE.md](INFERENCE.md)
for integration examples and a per-feature verification matrix. Management
bindings and live validation are described in [MANAGEMENT.md](MANAGEMENT.md).

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

## MCP sessions

Native request/response MCP sessions support initialization, session IDs, negotiated
protocol headers, JSON and finite SSE responses, notifications, tool discovery,
explicit tool calls, and session termination. Pagination is explicit. Use the
server's `nextCursor` with the next list request. Generic calls also support
resources, prompts, ping, and future JSON-RPC methods. Tool results containing
`isError` remain results so the caller can inspect their structured content.

Set `X-Erebine-Workspace` on the dedicated client's headers to scope MCP to an
explicit workspace; inference routing alone does not set MCP's workspace header.
No sampling, roots, elicitation, subscription listener, or reconnect/resume support
is advertised. No tools are called implicitly.

```rust,no_run
use erebine::{mcp::McpSession, Client, json};
# async fn example(client: Client) -> Result<(), Box<dyn std::error::Error>> {
let mut session = McpSession::connect(client).await?;
let tools = session.list_tools(None).await?;
// Explicit invocation only; inspect the discovered inputSchema first.
// session.call_tool(name, arguments).await? uses the discovered schema.
session.close().await?;
# Ok(()) }
```

`examples/live_smoke.rs` provides opt-in, bounded account validation via a loopback
SSH proxy. Its remote proxy must inject credentials; no account key is received
by the SDK process. It is not part of `cargo test`. See [live validation](LIVE_VALIDATION.md).

Run the bundled SSH validation runner against an existing Erebine OpenCode provider
configuration on your own host (workspace-scoped direct base URL required):

```sh
python3 scripts/validate_live.py --ssh-host YOUR_SSH_ALIAS --opencode-config /path/to/opencode.json --model YOUR_MODEL_ID --mcp-only
```

Omit `--mcp-only` to run bounded Chat Completions, Responses, and streaming checks;
those requests may incur inference charges. Use `--sudo` only when the remote
configuration requires it. The runner closes its temporary remote proxy on exit.
It does not modify the provider configuration or persist credentials locally.
