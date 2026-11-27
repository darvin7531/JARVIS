#[allow(dead_code)]
#[path = "../src/provider.rs"]
mod provider;

use crate::provider::{MockProvider, Provider, Request};
use std::{sync::atomic::AtomicBool, time::Duration};

#[test]
fn mock_streams_text_and_usage() {
    let provider = MockProvider::new("hello");
    let request = Request {
        model: "test",
        prompt: "hi",
        max_tokens: 8,
        max_output_bytes: 64,
        deadline: std::time::Instant::now() + Duration::from_secs(1),
    };
    let mut text = String::new();
    let usage = provider
        .generate(&request, &AtomicBool::new(false), &mut |chunk| {
            text.push_str(chunk)
        })
        .unwrap();
    assert_eq!(text, "hello");
    assert_eq!(usage.output_tokens, 1);
}
