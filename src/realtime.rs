//! Opt-in native WebSocket transcription, enabled with the `realtime` feature.
use crate::{json, Client, Error, Value};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
/// Text/binary/control frames stay available through tungstenite's Message type.
pub use tokio_tungstenite::tungstenite::Message as RealtimeMessage;
use tokio_tungstenite::{
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};
pub struct RealtimeSession {
    socket: WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
    pub headers: reqwest::header::HeaderMap,
}
fn socket_error(error: tokio_tungstenite::tungstenite::Error) -> Error {
    Error::Realtime(Box::new(error))
}
impl Client {
    /// Use an endpoint configured for task_mode=realtime. HTTP routing/credential
    /// headers are reused; redirects are never automatically followed.
    pub async fn realtime(&self) -> Result<RealtimeSession, Error> {
        self.realtime_with_query(&[]).await
    }
    pub async fn realtime_with_query(
        &self,
        query: &[(&str, &str)],
    ) -> Result<RealtimeSession, Error> {
        if self.surface != crate::Surface::Router {
            return Err(Error::Invalid("Realtime requires a router client".into()));
        }
        let mut request = self.request("GET", "/{project_id}/{endpoint_slug}/v1/realtime");
        for (name, value) in query {
            request = request.query(name, value);
        }
        let mut url = request.url()?;
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .map_err(|_| Error::Invalid("invalid WebSocket scheme".into()))?;
        let mut request = url.as_str().into_client_request().map_err(socket_error)?;
        request.headers_mut().extend(self.headers.clone());
        let (socket, response) = tokio::time::timeout(
            self.request_timeout,
            tokio_tungstenite::connect_async_with_config(
                request,
                Some(
                    tokio_tungstenite::tungstenite::protocol::WebSocketConfig::default()
                        .max_message_size(Some(8 * 1024 * 1024))
                        .max_frame_size(Some(8 * 1024 * 1024)),
                ),
                true,
            ),
        )
        .await
        .map_err(|_| Error::Stream("WebSocket handshake timed out".into()))?
        .map_err(socket_error)?;
        Ok(RealtimeSession {
            socket,
            headers: response.headers().clone(),
        })
    }
}
impl RealtimeSession {
    pub async fn send(&mut self, message: RealtimeMessage) -> Result<(), Error> {
        self.socket.send(message).await.map_err(socket_error)
    }
    pub async fn send_json<T: Serialize + ?Sized>(&mut self, body: &T) -> Result<(), Error> {
        self.send(Message::Text(serde_json::to_string(body)?.into()))
            .await
    }
    pub async fn configure_transcription(
        &mut self,
        model: &str,
        input_audio_format: &str,
    ) -> Result<(), Error> {
        self.send_json(&json!({"type":"session.update","session":{"type":"transcription","input_audio_format":input_audio_format,"input_audio_transcription":{"model":model}}})).await
    }
    pub async fn append_audio(&mut self, audio: &[u8]) -> Result<(), Error> {
        self.send_json(&json!({"type":"input_audio_buffer.append","audio":STANDARD.encode(audio)}))
            .await
    }
    pub async fn commit_audio(&mut self) -> Result<(), Error> {
        self.send_json(&json!({"type":"input_audio_buffer.commit"}))
            .await
    }
    pub async fn receive(&mut self) -> Result<Option<RealtimeMessage>, Error> {
        self.socket.next().await.transpose().map_err(socket_error)
    }
    /// Raw receive() also exposes ping/pong/close frames and non-JSON data.
    pub async fn receive_json(&mut self) -> Result<Option<Value>, Error> {
        loop {
            match self.receive().await? {
                Some(Message::Text(text)) => return Ok(Some(serde_json::from_str(&text)?)),
                Some(Message::Binary(bytes)) => return Ok(Some(serde_json::from_slice(&bytes)?)),
                Some(Message::Close(_)) | None => return Ok(None),
                _ => {}
            }
        }
    }
    pub async fn close(&mut self) -> Result<(), Error> {
        self.socket.close(None).await.map_err(socket_error)
    }
}
