//! Erebine MCP request/response sessions over Streamable HTTP (2025-06-18).
//! Supports JSON and finite SSE POST responses. No sampling, elicitation, roots,
//! automatic reconnection or client-side tool execution is advertised.
use crate::{json, operations, sse::Decoder, Client, Error, Request, Response, Value};

pub struct McpSession {
    client: Client,
    session_id: Option<String>,
    next_id: u64,
    closed: bool,
    pub initialization: Value,
}
impl McpSession {
    pub async fn connect(client: Client) -> Result<Self, Error> {
        let mut session = Self {
            client,
            session_id: None,
            next_id: 1,
            closed: false,
            initialization: Value::Null,
        };
        let response=session.request()?.json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"erebine-rs","version":env!("CARGO_PKG_VERSION")}}}))?.send().await?;
        session.session_id = response
            .headers
            .get("mcp-session-id")
            .and_then(|x| x.to_str().ok())
            .map(str::to_owned);
        let result = decode_response(&response, 1)?;
        if result["protocolVersion"] != "2025-06-18" {
            return Err(Error::Invalid(
                "unsupported negotiated MCP protocol version".into(),
            ));
        }
        session.initialization = result;
        session.next_id = 2;
        session.notify("notifications/initialized", None).await?;
        Ok(session)
    }
    fn request(&self) -> Result<Request<'_>, Error> {
        if self.closed {
            return Err(Error::Invalid("MCP session is closed".into()));
        }
        let mut request = self
            .client
            .operation(operations::MCP_POST)
            .header("accept", "application/json, text/event-stream")?;
        if let Some(id) = &self.session_id {
            request = request.header("mcp-session-id", id)?;
        }
        if !self.initialization.is_null() {
            request = request.header("mcp-protocol-version", "2025-06-18")?;
        }
        Ok(request)
    }
    /// Return the JSON-RPC result; unknown fields and tool input schemas survive.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("MCP request ID exhausted".into()))?;
        let response = self
            .request()?
            .json(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?
            .send()
            .await?;
        decode_response(&response, id)
    }
    pub async fn notify(&self, method: &str, params: Option<Value>) -> Result<(), Error> {
        let mut body = json!({"jsonrpc":"2.0","method":method});
        if let Some(params) = params {
            body["params"] = params;
        }
        let response = self.request()?.json(&body)?.send().await?;
        if response.status != 202 || !response.body.is_empty() {
            return Err(Error::Invalid(
                "MCP notification expected empty HTTP 202".into(),
            ));
        }
        Ok(())
    }
    /// Pagination is explicit: pass the server's nextCursor on the next call.
    pub async fn list_tools(&mut self, cursor: Option<&str>) -> Result<Value, Error> {
        self.call(
            "tools/list",
            cursor
                .map(|c| json!({"cursor":c}))
                .unwrap_or_else(|| json!({})),
        )
        .await
    }
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, Error> {
        self.call("tools/call", json!({"name":name,"arguments":arguments}))
            .await
    }
    pub async fn ping(&mut self) -> Result<Value, Error> {
        self.call("ping", json!({})).await
    }
    pub async fn list_prompts(&mut self, cursor: Option<&str>) -> Result<Value, Error> {
        self.call(
            "prompts/list",
            cursor
                .map(|c| json!({"cursor": c}))
                .unwrap_or_else(|| json!({})),
        )
        .await
    }
    pub async fn get_prompt(
        &mut self,
        name: &str,
        arguments: std::collections::BTreeMap<String, String>,
    ) -> Result<Value, Error> {
        self.call("prompts/get", json!({"name": name, "arguments": arguments}))
            .await
    }
    pub async fn list_resources(&mut self, cursor: Option<&str>) -> Result<Value, Error> {
        self.call(
            "resources/list",
            cursor
                .map(|c| json!({"cursor": c}))
                .unwrap_or_else(|| json!({})),
        )
        .await
    }
    pub async fn list_resource_templates(&mut self, cursor: Option<&str>) -> Result<Value, Error> {
        self.call(
            "resources/templates/list",
            cursor
                .map(|c| json!({"cursor": c}))
                .unwrap_or_else(|| json!({})),
        )
        .await
    }
    pub async fn read_resource(&mut self, uri: &str) -> Result<Value, Error> {
        self.call("resources/read", json!({"uri": uri})).await
    }
    pub async fn subscribe_resource(&mut self, uri: &str) -> Result<Value, Error> {
        self.call("resources/subscribe", json!({"uri": uri})).await
    }
    pub async fn unsubscribe_resource(&mut self, uri: &str) -> Result<Value, Error> {
        self.call("resources/unsubscribe", json!({"uri": uri}))
            .await
    }
    /// Open server notifications, or resume an event stream using its last ID.
    /// A server may refuse GET with 405. The caller owns reconnect/backoff and
    /// dispatch; this never reissues an RPC or runs a received server request.
    pub async fn events(&self, last_event_id: Option<&str>) -> Result<crate::EventStream, Error> {
        if self.closed {
            return Err(Error::Invalid("MCP session is closed".into()));
        }
        let mut request = self
            .client
            .operation(operations::MCP_GET)
            .header("mcp-protocol-version", "2025-06-18")?;
        if let Some(id) = &self.session_id {
            request = request.header("mcp-session-id", id)?;
        }
        if let Some(id) = last_event_id {
            request = request.header("last-event-id", id)?;
        }
        request.stream().await
    }
    /// Explicitly terminate this session. A server may refuse DELETE with 405.
    pub async fn close(&mut self) -> Result<(), Error> {
        if self.closed {
            return Ok(());
        }
        if let Some(id) = &self.session_id {
            self.client
                .operation(operations::MCP_DELETE)
                .header("mcp-session-id", id)?
                .header("mcp-protocol-version", "2025-06-18")?
                .send()
                .await?;
        }
        self.closed = true;
        Ok(())
    }
}
fn decode_response(response: &Response, id: u64) -> Result<Value, Error> {
    let kind = response
        .headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    let messages: Vec<Value> = match kind {
        "application/json" => vec![response.json()?],
        "text/event-stream" => Decoder::default()
            .push(&response.body)?
            .into_iter()
            .map(|e| e.json())
            .collect::<Result<_, _>>()?,
        _ => return Err(Error::Invalid("MCP expected JSON or SSE response".into())),
    };
    let message = messages
        .into_iter()
        .find(|m| m["id"] == id && (m.get("result").is_some() || m.get("error").is_some()))
        .ok_or_else(|| Error::Invalid("MCP response ID missing or mismatched".into()))?;
    if message["jsonrpc"] != "2.0" {
        return Err(Error::Invalid("MCP response is not JSON-RPC 2.0".into()));
    }
    if let Some(error) = message.get("error") {
        return Err(Error::Rpc(error.clone()));
    }
    Ok(message["result"].clone())
}
