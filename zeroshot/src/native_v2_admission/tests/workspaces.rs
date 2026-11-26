use super::*;
use openengine_cluster_testkit::assertions::AssertError;
use openengine_cluster_protocol::{RunWorkspace, WorkspacePlacement, RunProfileName};

fn spec() -> RunWorkspace {
    RunWorkspace {
        placement: WorkspacePlacement::Managed,
        local_directory: None,
        seeds: vec![],
        copy_root: false,
    }
}

#[tokio::test]
async fn workspace_admission_accepts_empty_environment_without_source_and_rejects_git_features() {
    let mut request = submission(graph(vec![succeed("done")]), BTreeMap::new());
    request.workspace = Some(spec());
    request.source = None;
    request.environment = Some(serde_json::from_value(json!({})).assert_value());
    let admitted = NativeV2Admission
        .admit(request.clone())
        .await
        .assert_value();
    assert_eq!(admitted.workspace, request.workspace);
    assert!(admitted.source.is_none());
    request.source = Some(resolved_source());
    assert!(
        NativeV2Admission
            .admit(request.clone())
            .await
            .assert_error()
            .to_string()
            .contains("workspace mode rejects")
    );
    request.source = None;
    request.environment = Some(
        serde_json::from_value(
            json!({"contextRepositories":[{"repository":"acme/library","branch":"main"}]}),
        )
        .assert_value(),
    );
    assert!(
        NativeV2Admission
            .admit(request)
            .await
            .assert_error()
            .to_string()
            .contains("contextRepositories")
    );
}

#[tokio::test]
async fn nested_saved_profile_delivery_is_rejected_after_expansion_without_source() {
    use crate::native_v2_admission::profile_expansion::expand_profile_imports;
    let mut parent = submission(
        graph(vec![
            json!({"kind":"profile","name":"outer","profile":"middle"}),
            succeed("done"),
        ]),
        BTreeMap::new(),
    );
    let middle = graph(vec![
        json!({"kind":"profile","name":"inner","profile":"leaf"}),
        succeed("done"),
    ]);
    let leaf = graph(vec![
        delivery_verifier("publish", DeliveryMode::PullRequest),
        succeed("done"),
    ]);
    let mut leaf_runtime = parent.runtime.clone();
    leaf_runtime
        .nodes_mut()
        .insert(named("publish"), delivery_binding());
    let catalog = BTreeMap::from([
        (
            RunProfileName::new("middle").assert_value(),
            (middle, parent.runtime.clone()),
        ),
        (
            RunProfileName::new("leaf").assert_value(),
            (leaf, leaf_runtime),
        ),
    ]);
    let (graph, runtime) = expand_profile_imports(parent.graph, parent.runtime, None, |name| {
        Ok(catalog.get(name).cloned())
    })
    .assert_value();
    parent.graph = graph;
    parent.runtime = runtime;
    parent.workspace = Some(spec());
    parent.source = None;
    let error = NativeV2Admission.admit(parent).await.assert_error();
    assert!(
        error
            .to_string()
            .contains("workspace mode does not support delivery"),
        "{error}"
    );
}
