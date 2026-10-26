//! Explicit, caller-owned Chat Completions stream assembly. Tools are never run.
use crate::{Error, Event, Value};
use std::collections::BTreeMap;
#[derive(Clone, Debug, Default)]
pub struct StreamToolCall {
    pub id: Option<String>,
    pub kind: Option<String>,
    pub name: String,
    pub arguments: String,
}
#[derive(Clone, Debug, Default)]
pub struct StreamChoice {
    pub role: Option<String>,
    pub content: String,
    pub reasoning: String,
    pub refusal: String,
    pub tool_calls: BTreeMap<u64, StreamToolCall>,
    pub finish_reason: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub struct ChatAccumulator {
    pub id: Option<String>,
    pub model: Option<String>,
    pub choices: BTreeMap<u64, StreamChoice>,
    pub usage: Option<Value>,
    /// Preserves every event body, including unknown vendor events and fields.
    /// Memory grows with the stream; use raw event consumption for bounded memory.
    pub raw: Vec<Value>,
}
impl ChatAccumulator {
    pub fn push(&mut self, event: &Event) -> Result<(), Error> {
        self.push_json(event.json()?)
    }
    pub fn push_json(&mut self, value: Value) -> Result<(), Error> {
        self.raw.push(value.clone());
        if value.get("error").is_some_and(|v| !v.is_null()) {
            return Err(Error::Stream(
                "server sent an inference error event; inspect the raw event".into(),
            ));
        }
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            self.id = Some(id.into());
        }
        if let Some(model) = value.get("model").and_then(Value::as_str) {
            self.model = Some(model.into());
        }
        if let Some(usage) = value.get("usage").filter(|v| !v.is_null()) {
            self.usage = Some(usage.clone());
        }
        if let Some(choices) = value.get("choices").and_then(Value::as_array) {
            for item in choices {
                let index = item.get("index").and_then(Value::as_u64).ok_or_else(|| {
                    Error::Invalid("stream choice requires an unsigned index".into())
                })?;
                let choice = self.choices.entry(index).or_default();
                let delta = &item["delta"];
                if let Some(role) = delta.get("role").and_then(Value::as_str) {
                    choice.role = Some(role.into());
                }
                append(&mut choice.content, delta.get("content"));
                append(
                    &mut choice.reasoning,
                    delta
                        .get("reasoning_content")
                        .filter(|v| v.is_string())
                        .or_else(|| delta.get("reasoning")),
                );
                append(&mut choice.refusal, delta.get("refusal"));
                if let Some(reason) = item.get("finish_reason").and_then(Value::as_str) {
                    choice.finish_reason = Some(reason.into());
                }
                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        let index = call.get("index").and_then(Value::as_u64).ok_or_else(|| {
                            Error::Invalid("stream tool call requires an unsigned index".into())
                        })?;
                        let tool = choice.tool_calls.entry(index).or_default();
                        if let Some(id) = call.get("id").and_then(Value::as_str) {
                            tool.id = Some(id.into());
                        }
                        if let Some(kind) = call.get("type").and_then(Value::as_str) {
                            tool.kind = Some(kind.into());
                        }
                        append(&mut tool.name, call["function"].get("name"));
                        append(&mut tool.arguments, call["function"].get("arguments"));
                    }
                }
            }
        }

        Ok(())
    }
}
fn append(target: &mut String, value: Option<&Value>) {
    if let Some(s) = value.and_then(Value::as_str) {
        target.push_str(s);
    }
}
