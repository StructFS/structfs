//! Provider-independent, incremental UTF-8 event framing.
//!
//! The byte ceiling covers the entire event, including comments and ignored
//! fields. CRLF, LF and CR delimit lines. A blank line dispatches data; EOF
//! dispatches a final data event even without a trailing blank line. Empty
//! data fields count. Errors are terminal. Frames completed before an error
//! remain in output order.

/// One dispatched server-sent event.
///
/// Construct with [`SseFrame::new`] and the `with_*` builders; the struct is
/// `#[non_exhaustive]` so new SSE fields can be added without a breaking
/// change.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct SseFrame {
    /// The `data` field, with multiple `data:` lines joined by newlines.
    pub data: String,

    /// The `event` field, if this event carried one.
    pub event: Option<String>,

    /// The last event ID *in effect* for this event.
    ///
    /// Per the SSE specification the last-event-ID buffer persists across
    /// dispatches: once an `id:` line is seen, every later event carries it
    /// until another `id:` replaces it. It is not consumed by a dispatch.
    pub id: Option<String>,

    /// A new reconnection time (milliseconds), when one was read since the
    /// previous dispatched frame.
    ///
    /// Per the WHATWG algorithm `retry:` takes effect when the field is
    /// *read*, not when an event dispatches — so a block holding only
    /// `retry:` (no data) is not lost: the value is carried to the next
    /// frame here, and is also available immediately, and persistently, from
    /// [`SseFramer::reconnection_time`]. This field is `None` on frames
    /// that did not change it.
    pub retry: Option<u64>,
}

impl SseFrame {
    /// A frame carrying only data.
    pub fn new(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            ..Default::default()
        }
    }

    /// Set the event type.
    #[must_use]
    pub fn with_event(mut self, event: impl Into<String>) -> Self {
        self.event = Some(event.into());
        self
    }

    /// Set the last event ID in effect.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the reconnection time in milliseconds.
    #[must_use]
    pub fn with_retry(mut self, retry: u64) -> Self {
        self.retry = Some(retry);
        self
    }
}

/// A terminal framing failure. Nothing is emitted after one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SseError {
    /// One event exceeded the configured byte ceiling.
    #[error("SSE frame exceeds the configured byte limit")]
    FrameTooLarge,

    /// A line was not valid UTF-8.
    #[error("SSE stream contains invalid UTF-8")]
    InvalidUtf8,
}

/// Incremental SSE framer: push bytes, take whole frames.
pub struct SseFramer {
    limit: usize,
    size: usize,
    line: Vec<u8>,
    data: Vec<String>,
    event: Option<String>,
    /// The last-event-ID buffer; never reset by a dispatch.
    id: Option<String>,
    /// A reconnection time read since the last dispatched frame.
    pending_retry: Option<u64>,
    /// The reconnection time currently in effect.
    reconnection_time: Option<u64>,
    skip_lf: bool,
    ended: bool,
}

impl SseFramer {
    /// A framer that rejects any single event larger than `max_frame_bytes`.
    pub fn new(max_frame_bytes: usize) -> Self {
        Self {
            limit: max_frame_bytes,
            size: 0,
            line: Vec::new(),
            data: Vec::new(),
            event: None,
            id: None,
            pending_retry: None,
            reconnection_time: None,
            skip_lf: false,
            ended: false,
        }
    }

    /// The reconnection time in effect, in milliseconds: the last valid
    /// `retry:` value read, whether or not any event has dispatched since.
    pub fn reconnection_time(&self) -> Option<u64> {
        self.reconnection_time
    }

    /// The last-event-ID buffer: what a client would send as
    /// `Last-Event-ID` when reconnecting. An empty `id:` line sets it to
    /// the empty string.
    pub fn last_event_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Feed a chunk of bytes, returning every frame it completed.
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

    /// Signal end of stream, dispatching any pending data.
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
                // Takes effect immediately, independent of dispatch.
                if let Ok(ms) = value.parse() {
                    self.reconnection_time = Some(ms);
                    self.pending_retry = Some(ms);
                }
            }
            _ => (),
        }

        Ok(None)
    }

    fn dispatch(&mut self) -> Option<SseFrame> {
        self.size = 0;
        let data = std::mem::take(&mut self.data);
        let event = self.event.take();

        if data.is_empty() {
            // Per the specification the data and event-type buffers reset
            // even when no event fires; the last-event-ID buffer does not,
            // and a reconnection time already took effect when it was read
            // — keep it pending for the next frame.
            return None;
        }

        let retry = self.pending_retry.take();

        Some(SseFrame {
            data: data.join("\n"),
            event,
            // The last event ID persists across dispatches: it is the value
            // a client would send as `Last-Event-ID` when reconnecting.
            id: self.id.clone(),
            retry,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_event_id_persists_across_frames() {
        let mut framer = SseFramer::new(256);
        let mut frames = framer.push(b"id: 1\ndata: a\n\ndata: b\n\n");
        frames.extend(framer.push(b"id: 2\ndata: c\n\ndata: d\n\n"));

        let ids: Vec<Option<&str>> = frames
            .iter()
            .map(|f| f.as_ref().unwrap().id.as_deref())
            .collect();
        assert_eq!(ids, vec![Some("1"), Some("1"), Some("2"), Some("2")]);
    }

    #[test]
    fn an_event_with_no_data_does_not_clear_the_last_event_id() {
        let mut framer = SseFramer::new(256);
        // A lone `id:` with no data dispatches nothing, but is remembered.
        assert!(framer.push(b"id: 7\n\n").is_empty());
        let frames = framer.push(b"data: later\n\n");
        assert_eq!(frames[0].as_ref().unwrap().id.as_deref(), Some("7"));
    }

    #[test]
    fn a_retry_only_block_is_not_lost() {
        let mut framer = SseFramer::new(256);
        // No data: nothing dispatches, but the retry takes effect at once.
        assert!(framer.push(b"retry: 3000\n\n").is_empty());
        assert_eq!(framer.reconnection_time(), Some(3000));

        // ... and is surfaced on the next frame, once.
        let frames = framer.push(b"data: a\n\ndata: b\n\n");
        assert_eq!(frames[0].as_ref().unwrap().retry, Some(3000));
        assert_eq!(frames[1].as_ref().unwrap().retry, None);
        // The value in effect persists.
        assert_eq!(framer.reconnection_time(), Some(3000));
    }

    #[test]
    fn an_empty_id_resets_the_last_event_id_to_empty() {
        let mut framer = SseFramer::new(256);
        let mut frames = framer.push(b"id: 5\ndata: a\n\n");
        frames.extend(framer.push(b"id\ndata: b\n\n"));
        frames.extend(framer.push(b"id:\ndata: c\n\n"));

        let ids: Vec<Option<&str>> = frames
            .iter()
            .map(|f| f.as_ref().unwrap().id.as_deref())
            .collect();
        assert_eq!(ids, vec![Some("5"), Some(""), Some("")]);
        assert_eq!(framer.last_event_id(), Some(""));
    }

    #[test]
    fn frame_builders_compose() {
        let frame = SseFrame::new("hello")
            .with_event("delta")
            .with_id("9")
            .with_retry(1500);
        assert_eq!(frame.data, "hello");
        assert_eq!(frame.event.as_deref(), Some("delta"));
        assert_eq!(frame.id.as_deref(), Some("9"));
        assert_eq!(frame.retry, Some(1500));
    }

    #[test]
    fn errors_render_as_prose_not_debug() {
        assert_eq!(
            SseError::FrameTooLarge.to_string(),
            "SSE frame exceeds the configured byte limit"
        );
        assert_eq!(
            SseError::InvalidUtf8.to_string(),
            "SSE stream contains invalid UTF-8"
        );
    }
}
