//! Optional typed helpers. JSON requests are always available for full wire fidelity.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Clone, Debug, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Value,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
impl Message {
    pub fn text(role: &str, text: &str) -> Self {
        Self {
            role: role.into(),
            content: Value::String(text.into()),
            extra: BTreeMap::new(),
        }
    }
    pub fn user(text: &str) -> Self {
        Self::text("user", text)
    }
    pub fn system(text: &str) -> Self {
        Self::text("system", text)
    }
}
#[derive(Clone, Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
impl ChatCompletionRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            extra: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatCompletion {
    pub id: String,
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Choice {
    pub index: u64,
    pub message: Value,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

impl Message {
    pub fn tool(call_id: &str, output: &str) -> Self {
        let mut message = Self::text("tool", output);
        message
            .extra
            .insert("tool_call_id".into(), Value::String(call_id.into()));
        message
    }
    pub fn assistant_tool_calls(calls: Vec<Value>) -> Self {
        let mut message = Self::text("assistant", "");
        message.content = Value::Null;
        message
            .extra
            .insert("tool_calls".into(), Value::Array(calls));
        message
    }
}
// Required typed fields take precedence over extras, identically to the Swift SDK.
macro_rules! wire_request {
    ($name:ident { $($field:ident : $kind:ty),+ $(,)? }) => {
        #[derive(Clone,Debug,Deserialize)]
        pub struct $name { $(pub $field: $kind,)+ #[serde(flatten)] pub extra: BTreeMap<String,Value> }
        impl Serialize for $name {
            fn serialize<S:serde::Serializer>(&self, serializer:S) -> Result<S::Ok,S::Error> {
                let mut object:serde_json::Map<String,Value>=self.extra.clone().into_iter().collect();
                $(object.insert(stringify!($field).into(),serde_json::to_value(&self.$field).map_err(serde::ser::Error::custom)?);)+
                object.serialize(serializer)
            }
        }
    };
}
wire_request!(ResponsesRequest {
    model: String,
    input: Value
});
impl ResponsesRequest {
    pub fn new(model: impl Into<String>, input: Value) -> Self {
        Self {
            model: model.into(),
            input,
            extra: BTreeMap::new(),
        }
    }
}
wire_request!(EmbeddingRequest {
    model: String,
    input: Value
});
impl EmbeddingRequest {
    pub fn new(model: impl Into<String>, input: Value) -> Self {
        Self {
            model: model.into(),
            input,
            extra: BTreeMap::new(),
        }
    }
}
wire_request!(RerankRequest {model:String,query:String,documents:Vec<Value>});
impl RerankRequest {
    pub fn new(model: impl Into<String>, query: impl Into<String>, documents: Vec<Value>) -> Self {
        Self {
            model: model.into(),
            query: query.into(),
            documents,
            extra: BTreeMap::new(),
        }
    }
}
wire_request!(ScoreRequest {model:String,text_1:String,text_2:Vec<String>});
impl ScoreRequest {
    pub fn new(model: impl Into<String>, text_1: impl Into<String>, text_2: Vec<String>) -> Self {
        Self {
            model: model.into(),
            text_1: text_1.into(),
            text_2,
            extra: BTreeMap::new(),
        }
    }
}
wire_request!(SpeechRequest {
    model: String,
    input: String,
    voice: String
});
impl SpeechRequest {
    pub fn new(
        model: impl Into<String>,
        input: impl Into<String>,
        voice: impl Into<String>,
    ) -> Self {
        Self {
            model: model.into(),
            input: input.into(),
            voice: voice.into(),
            extra: BTreeMap::new(),
        }
    }
}
wire_request!(SystemOneRequest {model:String,state:Value,questions:BTreeMap<String,Value>});
impl SystemOneRequest {
    pub fn new(model: impl Into<String>, state: Value, questions: BTreeMap<String, Value>) -> Self {
        Self {
            model: model.into(),
            state,
            questions,
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AudioFile {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}
impl AudioFile {
    pub fn new(
        filename: impl Into<String>,
        content_type: impl Into<String>,
        data: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            filename: filename.into(),
            content_type: content_type.into(),
            data: data.into(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct AudioTranscriptionRequest {
    pub model: String,
    pub file: AudioFile,
    pub fields: Vec<(String, String)>,
}
impl AudioTranscriptionRequest {
    pub fn new(model: impl Into<String>, file: AudioFile) -> Self {
        Self {
            model: model.into(),
            file,
            fields: vec![],
        }
    }
}

/// Extract visible text from a Chat Completion or Responses result without
/// conflating reasoning with the answer. The full JSON remains available.
pub fn output_text(value: &Value) -> String {
    if let Some(text) = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_str)
    {
        return text.into();
    }
    value
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "message")
        .filter_map(|item| item.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|part| part["type"] == "output_text")
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("")
}

impl Serialize for Message {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields: serde_json::Map<String, Value> = self.extra.clone().into_iter().collect();
        fields.insert("role".into(), Value::String(self.role.clone()));
        fields.insert("content".into(), self.content.clone());
        fields.serialize(serializer)
    }
}
impl Serialize for ChatCompletionRequest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields: serde_json::Map<String, Value> = self.extra.clone().into_iter().collect();
        fields.insert("model".into(), Value::String(self.model.clone()));
        fields.insert(
            "messages".into(),
            serde_json::to_value(&self.messages).map_err(serde::ser::Error::custom)?,
        );
        fields.serialize(serializer)
    }
}
