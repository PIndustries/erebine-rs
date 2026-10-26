//! Optional typed helpers. JSON requests are always available for full wire fidelity.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Clone, Debug, Serialize, Deserialize)]
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
#[derive(Clone, Debug, Serialize, Deserialize)]
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
