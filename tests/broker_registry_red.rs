use jarvis::tool_policy::{permits, Scope};

#[test]
fn inert_tool_is_registered_for_read_scope() {
    assert!(permits("inert", &[Scope::Read]));
}
