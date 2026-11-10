use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

async fn admitted(nodes: usize) -> AdmittedRun {
    let mut children = (0..nodes).map(|index| json!({
        "kind":"step", "name":format!("worker_{index}"), "worker":"worker.test@1",
        "instructions":"original worker context ".repeat(300),
        "input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1,
    })).collect::<Vec<_>>();
    children.push(json!({"kind":"verifier","name":"late_review","worker":"reviewer.test@1","instructions":"Original late reviewer authority marker.",
        "input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"attempts":1,"signals":{},"diagnostic":{"kind":"null"}}));
    children.push(json!({"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}));
    let graph = crate::native_v2_candidate::test_support::full_graph(children);
    let mut bindings = (0..nodes).map(|index| (format!("worker_{index}"), json!({
        "kind":"agent","model":"original-worker-model","effort":"high","sessionScope":"node_instance"
    }))).collect::<serde_json::Map<_, _>>();
    bindings.insert("late_review".to_owned(), json!({"kind":"agent","model":"original-review-model","sessionScope":"execution","connections":{"declared":["API_KEY"]}}));
    let submission = serde_json::from_value(json!({
        "title":"Complete original authority", "submissionKey":"definition-authority", "initialInput":null,"graph":graph,
        "runtime":{"harness":"codex","provider":"openai","size":"host","nodes":bindings}
    })).assert_value();
    crate::native_v2_admission::NativeV2Admission
        .admit(submission)
        .await
        .assert_value()
}

fn workspace() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().assert_value();
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).assert_value();
    (directory, workspace)
}

#[tokio::test]
async fn an_ordinary_definition_keeps_late_nodes_and_original_runtime_complete_inline() {
    let admitted = admitted(4).await;
    let (_directory, workspace) = workspace();
    assert!(
        DefinitionFile::prepare(&admitted, &workspace)
            .assert_value()
            .is_none()
    );
    let definition = document(&admitted);
    let input = super::super::context_budget::base(json!({"evidence":definition}));
    assert_eq!(
        input["evidence"]["authored_graph"],
        serde_json::to_value(&admitted.graph).assert_value()
    );
    assert_eq!(
        input["evidence"]["authored_runtime"],
        serde_json::to_value(&admitted.runtime).assert_value()
    );
    assert!(
        input["evidence"]["authored_graph"]
            .to_string()
            .contains("Original late reviewer authority marker")
    );
    assert_eq!(
        input["evidence"]["authored_runtime"]["nodes"]["late_review"]["model"],
        "original-review-model"
    );
}

#[tokio::test]
async fn a_large_definition_uses_a_complete_private_readonly_file_outside_source() {
    let admitted = admitted(40).await;
    let (_directory, workspace) = workspace();
    let artifact = DefinitionFile::prepare(&admitted, &workspace)
        .assert_value()
        .assert_value();
    let path = artifact.path().to_owned();
    assert!(!path.starts_with(&workspace));
    let private_parent =
        crate::execution::platform::open_directory(path.parent().assert_value()).assert_value();
    crate::execution::platform::validate_private_file(&private_parent).assert_value();
    let private_file = crate::execution::platform::open_readonly_file(&path).assert_value();
    crate::execution::platform::validate_private_file(&private_file).assert_value();
    // Windows validation above inspects protected current-user/SYSTEM ACLs and ownership.
    drop(private_parent);
    drop(private_file);
    assert!(
        std::fs::metadata(&path)
            .assert_value()
            .permissions()
            .readonly()
    );
    let complete: Value =
        serde_json::from_slice(&std::fs::read(&path).assert_value()).assert_value();
    assert_eq!(complete, document(&admitted));
    let mut evidence = complete.clone();
    reference(&mut evidence, &path);
    let input = super::super::context_budget::base(json!({"evidence":evidence}));
    assert_eq!(input["evidence"]["authored_definition_file"], json!(path));
    assert!(input["evidence"]["authored_graph"].is_null());
    assert!(
        super::super::context_budget::encoded_size(&input)
            < super::super::context_budget::MAX_INPUT_BYTES
    );
    assert!(artifact.unchanged());
    drop(artifact);
    assert!(!path.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_writable_local_checkout_does_not_require_a_writable_parent() {
    use std::os::unix::fs::PermissionsExt;
    let admitted = admitted(40).await;
    let (directory, workspace) = workspace();
    let original = std::fs::metadata(directory.path())
        .assert_value()
        .permissions();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o555))
        .assert_value();
    let result = DefinitionFile::prepare(&admitted, &workspace);
    // Restore before checking the result so a failed assertion cannot strand the fixture.
    std::fs::set_permissions(directory.path(), original).assert_value();
    let artifact = result.assert_value().assert_value();
    assert!(artifact.unchanged());
    if unsafe { libc::geteuid() } != 0 {
        assert!(!artifact.path().starts_with(directory.path()));
    }
    assert!(!artifact.path().starts_with(&workspace));
    let path = artifact.path().to_owned();
    drop(artifact);
    assert!(!path.exists());
}
