# Inference SDK

Native inference integration is available alongside project management bindings.
See [MANAGEMENT.md](MANAGEMENT.md) for the management validation milestone.

## Coverage and proof

| Inference feature | Native API | Local protocol tests | Live account check |
| --- | --- | --- | --- |
| Model listing and retrieval | yes | yes | passed in Rust and Swift |
| Chat Completions | typed or arbitrary JSON | yes | passed in both |
| Responses | typed or arbitrary JSON | yes | passed in both |
| Chat/Responses SSE | native async stream | yes | passed in both |
| Tool calling and tool-result messages | explicit caller-controlled functions | yes | forced function and result round trip passed in both |
| Strict JSON schema output | request fields preserved | yes | schema-constrained result decoded in both |
| Reasoning, refusal, parallel tool-call fragments, usage | explicit ChatAccumulator | yes | usage and ordinary streamed responses passed; interleaving/refusal fixtures only |
| Embeddings | text, batched, tokenized JSON; float/base64 | yes | two-item float and base64 batches passed in both |
| Reranking and scoring | separate request types/routes | yes | both passed in both SDKs |
| Transcription | multipart, repeated fields, raw response | yes | short synthetic WAV accepted in both |
| Translation | multipart, raw JSON/text/subtitle response | yes | not live-verified |
| Speech synthesis and speech SSE | binary response / async SSE | yes | no speech deployment in the discovered account catalog |
| System 1 decisions | typed state/questions request | yes | no System 1 deployment in the discovered catalog |
| Realtime transcription | native authenticated WebSocket | yes | no realtime deployment identified; WebSocket behavior tested locally |
| Vision / audio content in chat | arbitrary message content parts | JSON passthrough | not live-verified |
| Response/chat cancellation | explicit HTTP operation | URL construction/catalog | cancellation of an active live generation not verified |

These checks establish request/response and stream handling, not that every model
supports every feature. Use model IDs from the same routing base. The account
currently exposes generation, embeddings, reranking, and transcription models.
Use an endpoint configured for the task you request. The SDK does not provision
models, execute tools, rewrite prompts, or retry a generation automatically.

## Request fidelity

Typed request helpers have `extra` fields for every optional or vendor parameter:
max tokens, temperature, reasoning, structured output, tools, service tiers,
metadata, truncation, previous_response_id, stream_options, and future fields.
Required typed fields win over the same keys in extra. Arbitrary JSON / custom
Encodable or Serialize types remain supported when a helper is too restrictive.
Responses retain their status, headers, original bytes, and unknown fields.

`streamChat` / `stream_chat` and `streamResponses` / `stream_responses` set
`stream=true`. Speech SSE sets `stream_format=sse`. Other options remain as given.
Request usage explicitly with `stream_options.include_usage=true` when desired.

Transcription/translation accept an AudioFile and ordered form fields; repeated
names are retained. Binary speech output is returned as bytes, not decoded as
JSON. All file bodies are currently in memory; use the raw transport for custom
body handling. Non-JSON subtitles and text responses remain raw bytes/text.

Tool calls belong to your app. Accumulate argument fragments, validate arguments,
run only the tool your app authorizes, then send its result as a tool message.
Responses uses function_call_output input items; JSON input arrays are accepted.
No SDK helper automatically invokes a tool or loops back to the model.

ChatAccumulator separates visible text, reasoning, refusal, tool argument strings,
finish reasons, and usage per choice/tool index. It retains raw JSON events for
vendor compatibility. Its memory grows with the stream; consume raw events if
you need a bounded-memory path. Responses event JSON remains available directly,
including function arguments, output items, and completion/error events.

Realtime uses the documented transcription subset of the OpenAI-shaped protocol:
session.update, input_audio_buffer.append (base64), input_audio_buffer.commit,
and server transcript delta/completed/error events. It is not a spoken-reply
agent loop. The native helpers expose raw frames and arbitrary JSON events too.
Client HTTP timeouts apply to ordinary requests and the Rust WebSocket handshake;
apply your app's own cancellation/read deadline to long-lived sockets.

## Reproducible live checks

The optional runner uses a restricted loopback SSH proxy. The account key remains
on the SSH host; no key is printed or saved locally. Native SDK processes send
actual requests through the tunnel. Fake-credential tests independently check
Authorization headers, including WebSocket upgrades.

```sh
python3 scripts/validate_live.py --ssh-host YOUR_SSH_ALIAS --opencode-config /path/to/opencode.json --model YOUR_GENERATION_MODEL --embedding-model YOUR_EMBEDDING_MODEL --rerank-model YOUR_RERANK_MODEL --transcription-model YOUR_TRANSCRIPTION_MODEL
```

The optional role arguments enable their respective checks. Inference requests
are bounded to 128 output tokens, use store=false where supported, and contain
synthetic prompts/audio only. The test function returns a local synthetic result;
no external or server-owned tool is run. Tests may incur inference charges.
Use --mcp-only to perform only MCP session/tool discovery, or --sudo if the remote
configuration requires it. The temporary proxy shuts down on runner exit.

## Rust quickstart

```rust,no_run
use erebine::{Client, Routing, types::{ChatCompletionRequest, Message, output_text}};
# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let client = Client::router("YOUR_KEY", "YOUR_PROJECT", Routing::Direct {
    workspace: Some("YOUR_WORKSPACE".into()),
})?;
let request = ChatCompletionRequest::new("MODEL_ID_FROM_MODELS_LIST", vec![Message::user("Hello")]);
let response = client.inference().chat(&request)?.send().await?;
println!("{}", output_text(&response.json()?));
# Ok(()) }
```

All methods return the existing Request builder so you can set headers, query
parameters, or inspect the URL before sending. Use `stream_chat(...)?` followed
by `.stream().await?`, then consume with futures_util::StreamExt.

Enable native WebSocket transcription with the optional feature:

```toml
erebine-rs = { git = "https://github.com/PIndustries/erebine-rs", features = ["realtime"] }
```

Call `client.realtime().await?`, configure_transcription, append_audio, commit_audio,
and receive_json. Raw send/receive frames and handshake headers remain available.
The feature uses rustls and does not require a browser or Erebine CLI.
