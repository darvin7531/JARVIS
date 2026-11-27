use jarvis::inference::{route, Capability, DataClass, ModelProfile, RouteError, Tier};

#[test]
fn t0_never_routes_to_a_model_even_when_manually_selected() {
    let models = [ModelProfile {
        name: "model",
        tier: Tier::T1,
        capabilities: &[Capability::Text],
        allowed_data: &[DataClass::Public],
    }];
    assert!(matches!(
        route(
            &models,
            Tier::T0,
            Some("model"),
            Capability::Text,
            DataClass::Public
        ),
        Err(RouteError::ModelUnavailable)
    ));
}
