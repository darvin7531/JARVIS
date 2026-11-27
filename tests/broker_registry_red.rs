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

#[test]
fn inspectable_schema_matches_validator() {
    use jarvis::tool_policy::{tool_schema, validate_tool};
    use serde_json::{json, Value};

    assert_eq!(tool_schema("shell"), None);
    let schema = tool_schema("inert").unwrap();
    assert_eq!(schema, json!({"type":"object", "required":["text"], "additionalProperties":false, "properties":{"text":{"type":"string", "maxUtf8Bytes":1024}}}));
    let cases = vec![
        json!({"text":""}),
        json!({"text":"a".repeat(1024)}),
        json!({"text":"é".repeat(512)}),
        json!({"text":"é".repeat(513)}),
        json!({"text":"a".repeat(1025)}),
        json!({"text":1}),
        json!({"text":"x", "extra":true}),
        json!({}),
        Value::Null,
    ];
    for args in cases {
        let accepted_by_schema = args.as_object().is_some_and(|obj| {
            obj.len() == 1
                && obj.get("text").and_then(Value::as_str).is_some_and(|text| {
                    text.len() <= schema["properties"]["text"]["maxUtf8Bytes"].as_u64().unwrap() as usize
                })
        });
        assert_eq!(validate_tool("inert", &args).is_ok(), accepted_by_schema, "{args}");
    }
}
