use structfs_http::sse::{SseError, SseFramer};
#[test]
fn arbitrary_chunk_boundaries_preserve_utf8_multiline_and_fields() {
    let input =
        ": comment\r\ndata:hé\r\ndata: two\nevent: delta\nid: x\nretry: 42\n\ndata:last".as_bytes();
    for split in 0..=input.len() {
        let mut framer = SseFramer::new(256);
        let mut frames = framer.push(&input[..split]);
        frames.extend(framer.push(&input[split..]));
        frames.extend(framer.finish());
        assert_eq!(frames.len(), 2);
        let first = frames[0].as_ref().unwrap();
        assert_eq!(first.data, "hé\ntwo");
        assert_eq!(first.event.as_deref(), Some("delta"));
        assert_eq!(first.id.as_deref(), Some("x"));
        assert_eq!(first.retry, Some(42));
        assert_eq!(frames[1].as_ref().unwrap().data, "last");
    }
}
#[test]
fn completed_events_survive_later_errors_in_the_same_chunk() {
    let mut framer = SseFramer::new(12);
    let events = framer.push(b"data:ok\n\ndata:01234567890123456789\n\n");
    assert_eq!(events[0].as_ref().unwrap().data, "ok");
    assert_eq!(events[1], Err(SseError::FrameTooLarge));
    assert!(framer.push(b"data:ignored\n\n").is_empty());
    let mut framer = SseFramer::new(32);
    assert_eq!(
        framer.push(b"data:\xff\n"),
        vec![Err(SseError::InvalidUtf8)]
    );
    assert!(framer.finish().is_empty());
}
