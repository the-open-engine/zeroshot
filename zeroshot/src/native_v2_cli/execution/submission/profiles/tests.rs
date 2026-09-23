use openengine_cluster_protocol::{
    RunProfileListResult, RunProfileName, RunProfileScope, RunProfileSummary, RunTitle,
};
use openengine_cluster_testkit::admission::graph_fixture;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;

use super::*;
use crate::native_v2_cli::tests::support::{Call, FakeBackend};

fn reference(name: &str, qualifier: Option<ProfileQualifier>) -> ProfileReference {
    ProfileReference {
        qualifier,
        name: RunProfileName::new(name).assert_value(),
    }
}

fn summary(name: &str, scope: RunProfileScope, is_default: bool) -> RunProfileSummary {
    RunProfileSummary {
        id: format!("profile-{name}"),
        name: RunProfileName::new(name).assert_value(),
        scope,
        is_default,
    }
}

fn materialization_fixture() -> (GraphSpec, RuntimePlan) {
    let graph = crate::native_v2_cli::BuiltinGraphTemplate::SingleWorker
        .materialize(crate::native_v2_cli::TemplateDelivery::None)
        .assert_value();
    let runtime = serde_json::from_value(json!({
        "harness":"codex",
        "provider":"openai",
        "size":"medium",
        "nodes":{"worker":{"kind":"agent","model":"coverage-model"}}
    }))
    .assert_value();
    (graph, runtime)
}

#[tokio::test]
async fn management_contract_selects_and_materializes_profiles() {
    let named = reference("beta", None);
    for (reference, target, expected) in [
        (None, None, vec![None]),
        (
            None,
            Some("prod"),
            vec![
                None,
                Some(RunProfileScope::User),
                Some(RunProfileScope::Org),
            ],
        ),
        (
            Some(reference("alpha", Some(ProfileQualifier::Local))),
            None,
            vec![None],
        ),
        (
            Some(reference("alpha", Some(ProfileQualifier::Org))),
            Some("prod"),
            vec![Some(RunProfileScope::Org)],
        ),
    ] {
        assert_eq!(
            profile_scopes(reference.as_ref(), target).assert_value(),
            expected
        );
    }
    let remote_without_target = reference("alpha", Some(ProfileQualifier::User));
    assert!(matches!(
        profile_scopes(Some(&remote_without_target), None).assert_error(),
        NativeV2CliError::Usage(message) if message == "remote profile selectors require --target"
    ));

    let list = RunProfileListResult {
        profiles: vec![
            summary("alpha", RunProfileScope::User, true),
            summary("beta", RunProfileScope::User, false),
        ],
    };
    assert_eq!(
        select_profile(list.clone(), None)
            .assert_value()
            .name
            .as_str(),
        "alpha"
    );
    assert_eq!(
        select_profile(list.clone(), Some(&named))
            .assert_value()
            .name
            .as_str(),
        "beta"
    );
    assert!(select_profile(list, Some(&reference("missing", None))).is_none());
    assert!(profile_not_found(None).to_string().contains("no default"));
    assert!(
        profile_not_found(Some(&reference("missing", None)))
            .to_string()
            .contains("profile missing")
    );

    let directory = tempfile::tempdir().assert_value();
    let graph_path = directory.path().join("graph.json");
    let runtime_path = directory.path().join("runtime.json");
    let (graph, runtime) = materialization_fixture();
    std::fs::write(&graph_path, serde_json::to_vec(&graph).assert_value()).assert_value();
    std::fs::write(&runtime_path, serde_json::to_vec(&runtime).assert_value()).assert_value();
    let selection = RunGraph::File(graph_path.clone());
    let runtime_source = RunRuntime::Exact(runtime_path);
    let materialized = materialize_profile(&selection, &runtime_source)
        .await
        .assert_value();
    assert_eq!(materialized, (graph, runtime));

    let unsupported = graph_fixture("worker", json!({"kind":"null"}));
    std::fs::write(&graph_path, serde_json::to_vec(&unsupported).assert_value()).assert_value();
    assert!(matches!(
        materialize_profile(&selection, &runtime_source)
            .await
            .assert_error(),
        NativeV2CliError::Usage(message)
            if message.contains("openengine.graph.full/v1")
    ));
}

#[tokio::test]
async fn wave6_cli_contract_remote_profile_resolution_preserves_selector_and_payload() {
    let backend = FakeBackend::default();
    let run = RunCommand {
        target: Some("prod".to_owned()),
        title: RunTitle::new("Use a stored profile").assert_value(),
        input: "unused.json".into(),
        selection: RunSelection::Profile(Some(reference("beta", Some(ProfileQualifier::Org)))),
        repository: None,
        branch: None,
        revision: None,
        detach: true,
        validate_only: false,
        submission_key: None,
    };

    let resolved = resolve_run_profile(&run, &backend).await.assert_value();
    let selector = resolved.remote_selector.assert_value();
    assert_eq!(selector.scope, RunProfileScope::Org);
    assert_eq!(selector.name.as_str(), "beta");
    assert_eq!(
        resolved.graph.profile,
        openengine_cluster_protocol::GraphProfile::Full
    );
    assert!(matches!(resolved.runtime, RuntimePlan::Codex { .. }));
    assert!(matches!(
        backend.calls().as_slice(),
        [
            Call::ProfileList { target, request },
            Call::ProfileShow {
                target: shown_target,
                selector: shown,
            },
        ] if target.as_deref() == Some("prod")
            && request.scope == RunProfileScope::Org
            && shown_target == target
            && shown == &selector
    ));

    let missing = RunCommand {
        selection: RunSelection::Profile(Some(reference("missing", Some(ProfileQualifier::Org)))),
        ..run
    };
    assert!(matches!(
        resolve_run_profile(&missing, &backend).await,
        Err(NativeV2CliError::Usage(message)) if message == "profile missing was not found"
    ));
}
