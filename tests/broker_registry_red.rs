use jarvis::tool_policy::{permits, Scope};

#[test]
fn inert_tool_is_registered_for_read_scope() {
    assert!(permits("inert", &[Scope::Read]));
}

#[test]
fn fixed_registry_validates_typed_inert_arguments() {
    use jarvis::tool_policy::{validate_tool, BrokerError, RegisteredTool};
    use serde_json::json;
    assert_eq!(
        validate_tool("inert", &json!({"text":"hello"})),
        Ok(RegisteredTool::Inert {
            text: "hello".into()
        })
    );
    assert_eq!(
        validate_tool("shell", &json!({"text":"hello"})),
        Err(BrokerError::UnknownTool)
    );
    assert_eq!(
        validate_tool("inert", &json!({"text":"hello", "extra":1})),
        Err(BrokerError::InvalidArguments)
    );
    assert_eq!(
        validate_tool("inert", &json!({"text":1})),
        Err(BrokerError::InvalidArguments)
    );
}
