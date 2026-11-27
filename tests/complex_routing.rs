use jarvis::inference::{classify, Tier};

#[test]
fn complex_signals_route_to_heavy_without_prefix() {
    assert_eq!(
        classify("Design the architecture for a critical incident"),
        Tier::T3
    );
}
