//! Ergonomic inference requests. Builders keep per-request headers, query values,
//! raw responses, and all vendor fields available. No tools run on the client.
use crate::{
    multipart, operations, types::AudioTranscriptionRequest, Client, Error, Request, Value,
};
use serde::Serialize;
#[derive(Clone, Copy)]
pub struct Inference<'a> {
    client: &'a Client,
}
impl Client {
    pub fn inference(&self) -> Inference<'_> {
        Inference { client: self }
    }
}
impl<'a> Inference<'a> {
    fn json<T: Serialize + ?Sized>(
        &self,
        op: crate::Operation,
        body: &T,
    ) -> Result<Request<'a>, Error> {
        self.client.operation(op).json(body)
    }
    pub fn models(&self) -> Request<'a> {
        self.client.operation(operations::MODELS_LIST)
    }
    pub fn model(&self, id: &str) -> Request<'a> {
        self.client
            .operation(operations::GET_PROJECT_ID_ENDPOINT_SLUG_V1_MODELS_ID)
            .param("id", id)
    }
    pub fn chat<T: Serialize + ?Sized>(&self, body: &T) -> Result<Request<'a>, Error> {
        self.json(operations::CHAT_COMPLETIONS_CREATE, body)
    }
    pub fn responses<T: Serialize + ?Sized>(&self, body: &T) -> Result<Request<'a>, Error> {
        self.json(operations::RESPONSES_CREATE, body)
    }
    pub fn embeddings<T: Serialize + ?Sized>(&self, body: &T) -> Result<Request<'a>, Error> {
        self.json(operations::EMBEDDINGS_CREATE, body)
    }
    pub fn rerank<T: Serialize + ?Sized>(&self, body: &T) -> Result<Request<'a>, Error> {
        self.json(operations::POST_PROJECT_ID_ENDPOINT_SLUG_V1_RERANK, body)
    }
    pub fn score<T: Serialize + ?Sized>(&self, body: &T) -> Result<Request<'a>, Error> {
        self.json(operations::POST_PROJECT_ID_ENDPOINT_SLUG_V1_SCORE, body)
    }
    pub fn system_one<T: Serialize + ?Sized>(&self, body: &T) -> Result<Request<'a>, Error> {
        self.json(operations::POST_PROJECT_ID_ENDPOINT_SLUG_V1_SYSTEMONE, body)
    }
    pub fn speech<T: Serialize + ?Sized>(&self, body: &T) -> Result<Request<'a>, Error> {
        self.json(
            operations::POST_PROJECT_ID_ENDPOINT_SLUG_V1_AUDIO_SPEECH,
            body,
        )
    }
    pub fn stream_chat<T: Serialize + ?Sized>(&self, body: &T) -> Result<Request<'a>, Error> {
        self.json(
            operations::CHAT_COMPLETIONS_CREATE,
            &streaming(body, "stream", Value::Bool(true))?,
        )
    }
    pub fn stream_responses<T: Serialize + ?Sized>(&self, body: &T) -> Result<Request<'a>, Error> {
        self.json(
            operations::RESPONSES_CREATE,
            &streaming(body, "stream", Value::Bool(true))?,
        )
    }
    pub fn stream_speech<T: Serialize + ?Sized>(&self, body: &T) -> Result<Request<'a>, Error> {
        self.json(
            operations::POST_PROJECT_ID_ENDPOINT_SLUG_V1_AUDIO_SPEECH,
            &streaming(body, "stream_format", Value::String("sse".into()))?,
        )
    }
    fn audio(
        &self,
        op: crate::Operation,
        body: &AudioTranscriptionRequest,
    ) -> Result<Request<'a>, Error> {
        if body
            .fields
            .iter()
            .any(|(name, _)| name == "model" || name == "file")
        {
            return Err(Error::Invalid(
                "additional audio fields cannot replace model or file".into(),
            ));
        }
        let part = multipart::Part::bytes(body.file.data.clone())
            .file_name(body.file.filename.clone())
            .mime_str(&body.file.content_type)?;
        let mut form = multipart::Form::new()
            .text("model", body.model.clone())
            .part("file", part);
        for (name, value) in &body.fields {
            form = form.text(name.clone(), value.clone());
        }
        Ok(self.client.operation(op).multipart(form))
    }
    pub fn transcribe(&self, body: &AudioTranscriptionRequest) -> Result<Request<'a>, Error> {
        self.audio(
            operations::POST_PROJECT_ID_ENDPOINT_SLUG_V1_AUDIO_TRANSCRIPTIONS,
            body,
        )
    }
    pub fn translate(&self, body: &AudioTranscriptionRequest) -> Result<Request<'a>, Error> {
        self.audio(
            operations::POST_PROJECT_ID_ENDPOINT_SLUG_V1_AUDIO_TRANSLATIONS,
            body,
        )
    }
    pub fn cancel_chat(&self, id: &str) -> Request<'a> {
        self.client
            .operation(operations::POST_PROJECT_ID_ENDPOINT_SLUG_V1_CHAT_COMPLETIONS_ID)
            .param("id", id)
    }
    pub fn cancel_response(&self, id: &str) -> Request<'a> {
        self.client
            .operation(operations::POST_PROJECT_ID_ENDPOINT_SLUG_V1_RESPONSES_ID_CANCEL)
            .param("id", id)
    }
}
fn streaming<T: Serialize + ?Sized>(body: &T, key: &str, value: Value) -> Result<Value, Error> {
    let mut body = serde_json::to_value(body)?;
    body.as_object_mut()
        .ok_or_else(|| Error::Invalid("inference body must be a JSON object".into()))?
        .insert(key.into(), value);
    Ok(body)
}
