//! Native, asynchronous community SDK for Erebine.
//!
//! Every catalog operation returns a request builder supporting JSON, multipart,
//! binary bodies, repeated query parameters, headers, and SSE. Unknown JSON fields
//! remain available. See the repository coverage report for verification boundaries.
pub mod operations;
pub mod sse;
pub mod types;

use futures_core::Stream;
use futures_util::StreamExt;
pub use reqwest::multipart;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Method, Url,
};
use serde::{de::DeserializeOwned, Serialize};
pub use serde_json::{json, Value};
pub use sse::Event;
use std::{collections::BTreeMap, pin::Pin, time::Duration};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid configuration or request: {0}")]
    Invalid(String),
    #[error("HTTP transport failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("JSON decoding failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Erebine HTTP {status} (request {request_id:?})")]
    Api {
        status: u16,
        request_id: Option<String>,
        retry_after: Option<String>,
        body: Vec<u8>,
        headers: Box<HeaderMap>,
    },
    #[error("invalid SSE stream: {0}")]
    Stream(String),
}
impl Error {
    /// Decode the complete server error, including vendor and future fields.
    pub fn api_body(&self) -> Option<Value> {
        if let Self::Api { body, .. } = self {
            serde_json::from_slice(body).ok()
        } else {
            None
        }
    }
}

/// A single documented HTTP operation. No network calls happen during construction.
#[derive(Clone, Copy, Debug)]
pub struct Operation {
    pub name: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub surface: Surface,
    pub sources: &'static [&'static str],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Router,
    Dashboard,
}

#[derive(Clone, Debug)]
pub enum Routing {
    Endpoint(String),
    Direct { workspace: Option<String> },
    Semantic { workspace: String },
}
impl Routing {
    fn segments(&self) -> Vec<&str> {
        match self {
            Self::Endpoint(slug) => vec![slug],
            Self::Direct {
                workspace: Some(id),
            } => vec![id, "_direct"],
            Self::Direct { workspace: None } => vec!["_direct"],
            Self::Semantic { workspace } => vec![workspace, "_semantic", "router"],
        }
    }
}

/// Configure a dedicated client per origin/authentication surface.
/// Redirects are disabled: credentials never follow a redirect automatically.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    origin: Url,
    headers: HeaderMap,
    project: Option<String>,
    routing: Option<Routing>,
    surface: Surface,
    omit_project: bool,
}
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("origin", &self.origin)
            .field("surface", &self.surface)
            .finish_non_exhaustive()
    }
}
impl Client {
    pub fn new(origin: &str, surface: Surface) -> Result<Self, Error> {
        let origin =
            Url::parse(origin).map_err(|_| Error::Invalid("expected an HTTP(S) origin".into()))?;
        if !["https", "http"].contains(&origin.scheme())
            || origin.host_str().is_none()
            || origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || !origin.username().is_empty()
            || origin.password().is_some()
        {
            return Err(Error::Invalid(
                "origin must contain only scheme, host, and optional port".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(300))
            .build()?;
        Ok(Self {
            http,
            origin,
            headers: HeaderMap::new(),
            project: None,
            routing: None,
            surface,
            omit_project: false,
        })
    }
    pub fn router(api_key: &str, project: &str, routing: Routing) -> Result<Self, Error> {
        Ok(Self::new("https://api.erebine.ai", Surface::Router)?
            .bearer(api_key)?
            .project(project)
            .routing(routing))
    }
    pub fn bearer(mut self, key: &str) -> Result<Self, Error> {
        let mut value = HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| Error::Invalid("invalid bearer token".into()))?;
        value.set_sensitive(true);
        self.headers.insert(reqwest::header::AUTHORIZATION, value);
        Ok(self)
    }
    /// Supply session/CSRF credentials explicitly for dashboard routes; never logs values.
    pub fn header(mut self, name: &str, value: &str) -> Result<Self, Error> {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| Error::Invalid("invalid header name".into()))?;
        let mut value = HeaderValue::from_str(value)
            .map_err(|_| Error::Invalid("invalid header value".into()))?;
        value.set_sensitive(true);
        self.headers.insert(name, value);
        Ok(self)
    }
    pub fn project(mut self, project: &str) -> Self {
        self.project = Some(project.into());
        self
    }
    pub fn routing(mut self, routing: Routing) -> Self {
        self.routing = Some(routing);
        self
    }
    /// Custom domains documented by Erebine supply the project from their hostname.
    pub fn custom_domain(mut self) -> Self {
        self.omit_project = true;
        self
    }
    pub fn timeout(mut self, duration: Duration) -> Result<Self, Error> {
        self.http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(duration)
            .build()?;
        Ok(self)
    }
    pub fn operation(&self, operation: Operation) -> Request<'_> {
        Request {
            client: self,
            method: operation.method.into(),
            path: operation.path.into(),
            surface: Some(operation.surface),
            params: BTreeMap::new(),
            query: vec![],
            headers: HeaderMap::new(),
            body: Body::Empty,
        }
    }
    /// Escape hatch for a documented operation not yet in the catalog.
    /// Paths are origin-relative templates; full URLs and dot segments are rejected.
    pub fn request(&self, method: &str, path: &str) -> Request<'_> {
        Request {
            client: self,
            method: method.into(),
            path: path.into(),
            surface: None,
            params: BTreeMap::new(),
            query: vec![],
            headers: HeaderMap::new(),
            body: Body::Empty,
        }
    }
}

enum Body {
    Empty,
    Bytes(Vec<u8>),
    Multipart(multipart::Form),
}
pub struct Request<'a> {
    client: &'a Client,
    method: String,
    path: String,
    surface: Option<Surface>,
    params: BTreeMap<String, String>,
    query: Vec<(String, String)>,
    headers: HeaderMap,
    body: Body,
}
impl<'a> Request<'a> {
    pub fn param(mut self, name: &str, value: impl ToString) -> Self {
        self.params.insert(name.into(), value.to_string());
        self
    }
    /// Repeated query names are retained, in order.
    pub fn query(mut self, name: &str, value: impl ToString) -> Self {
        self.query.push((name.into(), value.to_string()));
        self
    }
    pub fn header(mut self, name: &str, value: &str) -> Result<Self, Error> {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| Error::Invalid("invalid header name".into()))?;
        let mut value = HeaderValue::from_str(value)
            .map_err(|_| Error::Invalid("invalid header value".into()))?;
        value.set_sensitive(true);
        self.headers.insert(name, value);
        Ok(self)
    }
    pub fn json<T: Serialize + ?Sized>(mut self, body: &T) -> Result<Self, Error> {
        self.body = Body::Bytes(serde_json::to_vec(body)?);
        self.headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        Ok(self)
    }
    pub fn bytes(mut self, bytes: impl Into<Vec<u8>>, content_type: &str) -> Result<Self, Error> {
        self.body = Body::Bytes(bytes.into());
        self.header("content-type", content_type)
    }
    pub fn multipart(mut self, form: multipart::Form) -> Self {
        self.headers.remove(reqwest::header::CONTENT_TYPE);
        self.body = Body::Multipart(form);
        self
    }
    /// Inspect the escaped URL before submitting (no credentials appear here).
    pub fn url(&self) -> Result<Url, Error> {
        if self.surface.is_some_and(|s| s != self.client.surface) {
            return Err(Error::Invalid(
                "operation and client authentication surfaces differ".into(),
            ));
        }
        if !self.path.starts_with('/')
            || self.path.starts_with("//")
            || self.path.contains(['?', '#', '\\'])
        {
            return Err(Error::Invalid(
                "expected an origin-relative path without query or fragment".into(),
            ));
        }
        let mut url = self.client.origin.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| Error::Invalid("invalid origin".into()))?;
            path.clear();
            for segment in self.path.split('/').skip(1) {
                if segment.starts_with('{') && segment.ends_with('}') {
                    let name = &segment[1..segment.len() - 1];
                    if name == "project_id" && self.client.omit_project {
                        continue;
                    }
                    if name == "endpoint_slug" && !self.params.contains_key(name) {
                        if let Some(routing) = &self.client.routing {
                            for part in routing.segments() {
                                validate_segment(part)?;
                                path.push(part);
                            }
                            continue;
                        }
                    }
                    let value = self
                        .params
                        .get(name)
                        .or_else(|| {
                            if name == "project_id" {
                                self.client.project.as_ref()
                            } else {
                                None
                            }
                        })
                        .ok_or_else(|| Error::Invalid(format!("missing path parameter {name}")))?;
                    if name == "relative_path" || name == "path" {
                        for part in value.split('/') {
                            validate_segment(part)?;
                            path.push(part);
                        }
                    } else {
                        validate_segment(value)?;
                        path.push(value);
                    }
                } else {
                    if segment.contains(['{', '}']) {
                        return Err(Error::Invalid("malformed path template".into()));
                    }
                    validate_segment(segment)?;
                    path.push(segment);
                }
            }
        }
        if !self.query.is_empty() {
            url.query_pairs_mut().extend_pairs(&self.query);
        }
        Ok(url)
    }
    fn build(self) -> Result<reqwest::RequestBuilder, Error> {
        let url = self.url()?;
        let method = Method::from_bytes(self.method.as_bytes())
            .map_err(|_| Error::Invalid("invalid HTTP method".into()))?;
        let mut headers = self.client.headers.clone();
        headers.extend(self.headers);
        let request = self.client.http.request(method, url).headers(headers);
        Ok(match self.body {
            Body::Empty => request,
            Body::Bytes(b) => request.body(b),
            Body::Multipart(form) => request.multipart(form),
        })
    }
    pub async fn send(self) -> Result<Response, Error> {
        let response = self.build()?.send().await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let body = response.bytes().await?.to_vec();
        if !(200..300).contains(&status) {
            return Err(api_error(status, headers, body));
        }
        Ok(Response {
            status,
            headers,
            body,
        })
    }
    /// Streaming body must be requested in the JSON payload where required.
    /// Dropping this stream drops the HTTP response; use the API cancellation operation
    /// as well if the server-side generation must be stopped.
    pub async fn stream(self) -> Result<EventStream, Error> {
        let response = self
            .header("accept", "text/event-stream")?
            .build()?
            .send()
            .await?;
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        if !(200..300).contains(&status) {
            return Err(api_error(status, headers, response.bytes().await?.to_vec()));
        }
        if !headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|s| {
                s.split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .eq_ignore_ascii_case("text/event-stream")
            })
        {
            return Err(Error::Stream(
                "expected text/event-stream content type".into(),
            ));
        }
        let mut chunks = response.bytes_stream();
        let stream = async_stream::try_stream! {
            let mut parser = sse::Decoder::default();
            while let Some(chunk) = chunks.next().await {
                for event in parser.push(&chunk?)? {
                    if event.data == "[DONE]" { return; }
                    yield event;
                }
            }
            for event in parser.finish()? {
                if event.data == "[DONE]" { return; }
                yield event;
            }
        };
        Ok(EventStream {
            status,
            headers,
            inner: Box::pin(stream),
        })
    }
}
fn validate_segment(value: &str) -> Result<(), Error> {
    if value.is_empty() || value == "." || value == ".." || value.chars().any(|c| c.is_control()) {
        return Err(Error::Invalid(
            "empty, dot, or control-character path segment".into(),
        ));
    }
    Ok(())
}
fn api_error(status: u16, headers: HeaderMap, body: Vec<u8>) -> Error {
    let request_id = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let retry_after = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    Error::Api {
        status,
        request_id,
        retry_after,
        body,
        headers: Box::new(headers),
    }
}
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}
impl Response {
    pub fn json<T: DeserializeOwned>(&self) -> Result<T, Error> {
        Ok(serde_json::from_slice(&self.body)?)
    }
    pub fn text(&self) -> Result<&str, Error> {
        std::str::from_utf8(&self.body)
            .map_err(|_| Error::Invalid("response body is not UTF-8".into()))
    }
    pub fn request_id(&self) -> Option<&str> {
        self.headers
            .get("x-request-id")
            .and_then(|v| v.to_str().ok())
    }
}
pub struct EventStream {
    pub status: u16,
    pub headers: HeaderMap,
    inner: Pin<Box<dyn Stream<Item = Result<Event, Error>> + Send>>,
}
impl Stream for EventStream {
    type Item = Result<Event, Error>;
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}
