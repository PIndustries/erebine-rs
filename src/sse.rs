//! Incremental SSE decoder. Handles arbitrary byte boundaries, UTF-8, LF/CRLF/CR,
//! comments, multiline data, persistent IDs, retry fields, and unknown event names.
use crate::{Error, Value};
const MAX_EVENT: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub event: Option<String>,
    pub data: String,
    pub id: Option<String>,
    pub retry: Option<u64>,
}
impl Event {
    pub fn json(&self) -> Result<Value, Error> {
        Ok(serde_json::from_str(&self.data)?)
    }
}
#[derive(Default)]
pub struct Decoder {
    line: Vec<u8>,
    data: String,
    event: Option<String>,
    id: Option<String>,
    retry: Option<u64>,
    skip_lf: bool,
    started: bool,
    size: usize,
}
impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Event>, Error> {
        let mut events = Vec::new();
        for &byte in bytes {
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if byte == b'\r' || byte == b'\n' {
                self.process_line(&mut events)?;
                self.skip_lf = byte == b'\r';
            } else {
                self.line.push(byte);
                if self.line.len() + self.size > MAX_EVENT {
                    return Err(Error::Stream("SSE event exceeds 8 MiB".into()));
                }
            }
        }
        Ok(events)
    }
    fn process_line(&mut self, events: &mut Vec<Event>) -> Result<(), Error> {
        let bytes = std::mem::take(&mut self.line);
        let mut line =
            std::str::from_utf8(&bytes).map_err(|_| Error::Stream("invalid UTF-8".into()))?;
        if !self.started {
            line = line.trim_start_matches('\u{feff}');
            self.started = true;
        }
        if line.is_empty() {
            if !self.data.is_empty() {
                self.data.pop();
                events.push(Event {
                    event: self.event.take(),
                    data: std::mem::take(&mut self.data),
                    id: self.id.clone(),
                    retry: self.retry.take(),
                });
            }
            self.event = None;
            self.retry = None;
            self.size = 0;
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        self.size += line.len();
        if self.size > MAX_EVENT {
            return Err(Error::Stream("SSE event exceeds 8 MiB".into()));
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
            }
            "event" => self.event = Some(value.into()),
            "id" if !value.contains('\0') => self.id = Some(value.into()),
            "retry" if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
                self.retry = value.parse().ok()
            }
            _ => {}
        }
        Ok(())
    }
    /// Per the SSE specification, EOF discards an event lacking a blank terminator.
    pub fn finish(&mut self) -> Result<Vec<Event>, Error> {
        self.line.clear();
        self.data.clear();
        Ok(vec![])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_byte_boundaries() {
        let input = "\u{feff}: heartbeat\r\nid: 7\r\nevent: x_reasoning\r\nretry: 123\r\ndata: héllo\r\ndata: 🌎\r\n\r\ndata: next\n\n";
        let mut d = Decoder::default();
        let mut e = vec![];
        for b in input.as_bytes() {
            e.extend(d.push(&[*b]).unwrap());
        }
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].data, "héllo\n🌎");
        assert_eq!(e[0].event.as_deref(), Some("x_reasoning"));
        assert_eq!(e[0].retry, Some(123));
        assert_eq!(e[1].id.as_deref(), Some("7"));
    }
    #[test]
    fn empty_data_and_cr_and_eof() {
        let mut d = Decoder::default();
        let e = d.push(b"data:\r\rdata: unfinished").unwrap();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].data, "");
        assert!(d.finish().unwrap().is_empty());
    }
    #[test]
    fn rejects_invalid_utf8_and_oversize() {
        assert!(Decoder::default().push(&[255, 10]).is_err());
        assert!(Decoder::default().push(&vec![b'x'; MAX_EVENT + 1]).is_err());
    }
}
