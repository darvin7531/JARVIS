use jarvis::inference::{classify, Tier};

#[test]
fn rules_classify_tasks_before_model_selection() {
    assert_eq!(classify("hello"), Tier::T1);
    assert_eq!(classify("code: fix the bug"), Tier::T2);
    assert_eq!(classify("complex: analyze architecture"), Tier::T3);
}
