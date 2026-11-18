//! Fresh launch storage ownership and preparation failure regressions.

use super::*;

#[tokio::test]
async fn fresh_start_failure_after_storage_creation_keeps_the_original_error() {
    let root = TestDirectory::new("fresh-setup");
    let state = root.child("state");
    crate::execution::platform::create_private_directory(&state).assert_value();
    let backend = contract_backend(&state);
    let workspace = root.child("workspace");
    std::fs::create_dir(&workspace).assert_value();
    // Directory preparation fails after the recovery document has been written.
    let obstruction = state.join("workspaces");
    std::fs::write(&obstruction, b"keep").assert_value();
    let expected = backend.workspace_lease(&workspace).unwrap_err().to_string();
    let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000063");

    let error = backend
        .start_prepared_controller(prepared_local_run(
            run_id.clone(),
            contract_submission("fresh-setup"),
            workspace,
        ))
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), expected);
    assert!(!backend.run_storage(&run_id).assert_value().exists());
    assert_eq!(std::fs::read(obstruction).assert_value(), b"keep");
}

#[tokio::test]
async fn fresh_start_never_removes_storage_it_did_not_create() {
    let root = TestDirectory::new("fresh-existing");
    let state = root.child("state");
    crate::execution::platform::create_private_directory(&state).assert_value();
    let backend = contract_backend(&state);
    let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000064");
    let storage = backend.create_run_storage(&run_id).assert_value();
    let retained = storage.join("retained");
    std::fs::write(&retained, b"keep").assert_value();
    let expected = backend.create_run_storage(&run_id).unwrap_err().to_string();

    let error = backend
        .start_prepared_controller(prepared_local_run(
            run_id,
            contract_submission("fresh-existing"),
            root.path().to_owned(),
        ))
        .await
        .unwrap_err();

    assert_eq!(error.to_string(), expected);
    assert_eq!(std::fs::read(retained).assert_value(), b"keep");
}
