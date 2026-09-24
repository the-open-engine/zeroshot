use std::collections::BTreeSet;

use super::*;

#[test]
fn constructors_preserve_the_transport_boundary_for_every_case_shape() {
    for expected in [
        Expected::Initialize,
        Expected::EmptyGet,
        Expected::Error {
            code: -32600,
            domain: None,
            id: Some(1),
        },
    ] {
        let case = CaseDefinition::dispatched(
            "constructor-check",
            ConformanceModule::Dispatch,
            "{}",
            expected,
        );
        assert_eq!(case.requirement, ConformanceRequirement::Required);
        assert_eq!(
            case.applicability,
            if matches!(expected, Expected::Initialize | Expected::EmptyGet) {
                TYPED_DISPATCHED
            } else {
                WIRE_ONLY
            }
        );
    }

    for (expected, requirement, input) in [
        (
            Expected::WatchEstablished,
            ConformanceRequirement::Required,
            r#"{"jsonrpc":"2.0","id":1,"method":"watch","params":{}}"#,
        ),
        (
            Expected::LogsEstablished,
            ConformanceRequirement::Optional(OptionalCapability::Logs),
            r#"{"jsonrpc":"2.0","id":1,"method":"logs","params":{}}"#,
        ),
        (
            Expected::AgentAttachNotFound,
            ConformanceRequirement::Optional(OptionalCapability::AgentAttach),
            r#"{"jsonrpc":"2.0","id":1,"method":"agent/attach","params":{"execution":"portable-conformance-unknown"}}"#,
        ),
    ] {
        let case = CaseDefinition::direct(
            "constructor-check",
            ConformanceModule::Watch,
            requirement,
            expected,
        );
        assert_eq!(case.requirement, requirement);
        assert_eq!(case.applicability, INTERCEPTED);
        assert_eq!(case.input, input);
    }

    let unsupported = CaseDefinition::direct(
        "unsupported-direct-shape",
        ConformanceModule::Watch,
        ConformanceRequirement::Required,
        Expected::Initialize,
    );
    assert!(unsupported.input.is_empty());
}

#[test]
fn public_catalog_is_unique_complete_and_capability_honest() {
    let catalog = conformance_catalog();
    let unique_ids = catalog
        .iter()
        .map(ConformanceCase::id)
        .collect::<BTreeSet<_>>();
    assert_eq!(unique_ids.len(), catalog.len());
    assert!(catalog.iter().all(|case| !case.input().is_empty()));

    let modules = [
        (ConformanceModule::Initialize, 1),
        (ConformanceModule::Dispatch, 6),
        (ConformanceModule::Get, 1),
        (ConformanceModule::Admission, 2),
        (ConformanceModule::Lifecycle, 5),
        (ConformanceModule::Watch, 1),
        (ConformanceModule::Logs, 1),
        (ConformanceModule::AgentAttach, 1),
    ];
    for (module, expected_count) in modules {
        assert_eq!(
            catalog
                .iter()
                .filter(|case| case.module() == module)
                .count(),
            expected_count,
            "catalog coverage drifted for {module:?}"
        );
    }

    let optional = catalog
        .iter()
        .filter_map(|case| match case.requirement() {
            ConformanceRequirement::Required => None,
            ConformanceRequirement::Optional(capability) => Some((case.module(), capability)),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        optional,
        [
            (ConformanceModule::Logs, OptionalCapability::Logs),
            (
                ConformanceModule::AgentAttach,
                OptionalCapability::AgentAttach,
            ),
        ]
    );

    for case in catalog {
        let applicability = case.transport_applicability();
        assert!(applicability.ndjson && applicability.websocket);
        assert_eq!(
            applicability.dispatcher,
            !matches!(
                case.module(),
                ConformanceModule::Watch | ConformanceModule::Logs | ConformanceModule::AgentAttach
            )
        );
    }
}
