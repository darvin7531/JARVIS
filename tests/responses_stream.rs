use jarvis::inference::{ResponseStream, StreamError};

#[test]
fn decodes_fragmented_events_and_requires_completion() {
    let mut stream = ResponseStream::new(1024);
    let mut text = String::new();
    stream.push(b"event: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi", &mut text).unwrap();
    assert_eq!(text, "");
    stream.push(b"!\"}\r\n\r\n", &mut text).unwrap();
    assert_eq!(text, "Hi!");
    assert_eq!(stream.finish(), Err(StreamError::Truncated));
}
