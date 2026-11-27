use jarvis::inference::{route, Capability, DataClass, ModelProfile, RouteError, Tier};

#[test]
fn duplicate_manual_names_fail_closed() {
    let models = [
        ModelProfile {
            name: "same",
            tier: Tier::T1,
            capabilities: &[Capability::Text],
            allowed_data: &[DataClass::Public],
        },
        ModelProfile {
            name: "same",
            tier: Tier::T2,
            capabilities: &[Capability::Text],
            allowed_data: &[DataClass::Public],
        },
    ];
    assert!(matches!(
        route(
            &models,
            Tier::T1,
            Some("same"),
            Capability::Text,
            DataClass::Public
        ),
        Err(RouteError::ModelUnavailable)
    ));
}

#[test]
fn t0_profile_cannot_be_selected_as_a_model() {
    let models = [ModelProfile {
        name: "local",
        tier: Tier::T0,
        capabilities: &[Capability::Text],
        allowed_data: &[DataClass::Public],
    }];
    assert!(matches!(
        route(
            &models,
            Tier::T1,
            Some("local"),
            Capability::Text,
            DataClass::Public
        ),
        Err(RouteError::ModelUnavailable)
    ));
}
