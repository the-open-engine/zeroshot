use super::*;
use crate::native_v2_candidate::test_support::TestDirectory;
use openengine_cluster_testkit::assertions::AssertValue;

#[tokio::test]
async fn refused_managed_cleanup_retains_live_admitted_and_unknown_ownership() {
    for ownership in ["unadmitted", "live", "admitted", "unknown"] {
        let root = TestDirectory::new("refused-workspace-ownership");
        let source = root.child("source");
        std::fs::create_dir(&source).assert_value();
        std::fs::write(source.join("seed"), b"original input").assert_value();
        let backend = LocalCliBackend::new(
            root.child("state"),
            PathBuf::from("zeroshot"),
            source.clone(),
            PathBuf::from("git"),
        );
        let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000001");
        let mut submission =
            super::super::local_contract_tests::contract_submission("refused-workspace");
        submission.source = None;
        submission.workspace = Some(
            serde_json::from_value(serde_json::json!({
                "placement":"managed", "copyRoot":true
            }))
            .assert_value(),
        );
        let files =
            crate::native_v2_cli::files::workspace_files(&backend.state_root).assert_value();
        files
            .freeze(crate::native_v2_files::FreezeWorkspace {
                run_id: &run_id,
                submission_key: &submission.submission_key,
                workspace: submission.workspace.as_ref().unwrap(),
                local_root: Some(&source),
            })
            .assert_value();
        let destination = root.child("managed");
        std::fs::create_dir(&destination).assert_value();
        files.materialize(&run_id, &destination).assert_value();
        let paths = backend.paths(&run_id).assert_value();
        let _lease = if ownership == "live" {
            Some(ControllerLease::acquire(paths.lease()).assert_value())
        } else {
            None
        };
        if ownership == "admitted" {
            super::super::local_contract_tests::create_contract_run(&backend, &run_id, &submission)
                .await;
        } else if ownership == "unknown" {
            backend.create_run_storage(&run_id).assert_value();
            std::fs::write(paths.ledger(), b"unreadable ledger").assert_value();
        }
        backend
            .discard_unadmitted_workspace(&run_id, Some(&destination))
            .await
            .assert_value();
        let retained = ownership != "unadmitted";
        assert_eq!(destination.exists(), retained, "{ownership}");
        let restored = root.child("restored");
        std::fs::create_dir(&restored).assert_value();
        assert_eq!(
            files.materialize(&run_id, &restored).is_ok(),
            retained,
            "{ownership}"
        );
        assert_eq!(
            std::fs::read(source.join("seed")).assert_value(),
            b"original input"
        );
    }
}
