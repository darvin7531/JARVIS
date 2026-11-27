#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Read,
    Write,
}

struct Tool {
    name: &'static str,
    scope: Scope,
}

fn call(registry: &[Tool], name: &str, allowed: &[Scope], execute: &mut impl FnMut()) -> bool {
    if registry
        .iter()
        .any(|tool| tool.name == name && allowed.contains(&tool.scope))
    {
        execute();
        true
    } else {
        false
    }
}

#[test]
fn broker_executes_allowed_tool_but_denies_other_scopes() {
    let registry = [
        Tool {
            name: "read",
            scope: Scope::Read,
        },
        Tool {
            name: "write",
            scope: Scope::Write,
        },
    ];
    let mut calls = 0;
    let mut execute = || calls += 1;
    assert!(call(&registry, "read", &[Scope::Read], &mut execute));
    assert!(!call(&registry, "write", &[Scope::Read], &mut execute));
    assert!(!call(&registry, "unknown", &[Scope::Read], &mut execute));
    assert_eq!(calls, 1);
}
