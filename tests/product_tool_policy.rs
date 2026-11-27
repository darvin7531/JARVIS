use jarvis::tool_policy::{permits, Scope};

#[test]
fn product_policy_permits_only_allowed_registered_scopes() {
    assert!(permits("read", &[Scope::Read]));
    assert!(!permits("write", &[Scope::Read]));
    assert!(!permits("unknown", &[Scope::Read]));
}
