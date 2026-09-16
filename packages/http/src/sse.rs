//! Provider-independent, incremental UTF-8 event framing.
//! The byte ceiling covers the entire event, including comments/ignored fields.
//! CRLF, LF and CR delimit lines. A blank line dispatches data; EOF dispatches a
//! final data event even without a trailing blank line. Empty data fields count.
//! Errors are terminal. Frames completed before an error remain in output order.
use std::fmt;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFrame {
    pub data: String,
    pub event: Option<String>,
    pub id: Option<String>,
    pub retry: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseError {
    FrameTooLarge,
    InvalidUtf8,
}
impl fmt::Display for SseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SseError {}
pub struct SseFramer {
    limit: usize,
    size: usize,
    line: Vec<u8>,
    data: Vec<String>,
    event: Option<String>,
    id: Option<String>,
    retry: Option<u64>,
    skip_lf: bool,
    ended: bool,
}
impl SseFramer {
    pub fn new(max_frame_bytes: usize) -> Self {
        Self {
            limit: max_frame_bytes,
            size: 0,
            line: Vec::new(),
            data: Vec::new(),
            event: None,
            id: None,
            retry: None,
            skip_lf: false,
            ended: false,
        }
    }
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<SseFrame, SseError>> {
        let mut output = Vec::new();
        for &byte in bytes {
            if self.ended {
                break;
            }
            if self.skip_lf {
                self.skip_lf = false;
                if byte == b'\n' {
                    continue;
                }
            }
            if self.size == self.limit {
                self.ended = true;
                self.line.clear();
                self.data.clear();
                output.push(Err(SseError::FrameTooLarge));
                break;
            }
            self.size += 1;
            if byte == b'\r' || byte == b'\n' {
                self.skip_lf = byte == b'\r';
                match self.end_line() {
                    Ok(Some(frame)) => output.push(Ok(frame)),
                    Ok(None) => (),
                    Err(error) => {
                        self.ended = true;
                        output.push(Err(error));
                    }
                }
            } else {
                self.line.push(byte);
            }
        }
        output
    }
    pub fn finish(&mut self) -> Vec<Result<SseFrame, SseError>> {
        if self.ended {
            return Vec::new();
        }
        self.ended = true;
        if !self.line.is_empty() {
            if let Err(error) = self.end_line() {
                return vec![Err(error)];
            }
        }
        self.dispatch().map(|f| vec![Ok(f)]).unwrap_or_default()
    }
    fn end_line(&mut self) -> Result<Option<SseFrame>, SseError> {
        let bytes = std::mem::take(&mut self.line);
        let line = std::str::from_utf8(&bytes).map_err(|_| SseError::InvalidUtf8)?;
        if line.is_empty() {
            return Ok(self.dispatch());
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => self.data.push(value.to_owned()),
            "event" => self.event = Some(value.to_owned()),
            "id" if !value.contains('\0') => self.id = Some(value.to_owned()),
            "retry" if !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()) => {
                self.retry = value.parse().ok()
            }
            _ => (),
        }
        Ok(None)
    }
    fn dispatch(&mut self) -> Option<SseFrame> {
        self.size = 0;
        let data = std::mem::take(&mut self.data);
        let event = self.event.take();
        let id = self.id.take();
        let retry = self.retry.take();
        if data.is_empty() {
            None
        } else {
            Some(SseFrame {
                data: data.join("\n"),
                event,
                id,
                retry,
            })
        }
    }
}
