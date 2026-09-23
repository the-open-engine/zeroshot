use super::*;
use openengine_cluster_testkit::assertions::AssertValue;

use super::local_contract_tests::contract_submission;
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::v2_run_ledger::CreateRun;

fn backend(root: &Path) -> LocalCliBackend {
    LocalCliBackend::new(
        root.to_owned(),
        PathBuf::from("zeroshot"),
        root.to_owned(),
        PathBuf::from("git"),
    )
}

struct ResumeClaimFixture {
    _root: TestDirectory,
    backend: LocalCliBackend,
    run_id: RunId,
    successor_run_id: RunId,
    submission: RunSubmission,
    successor_storage: PathBuf,
}

fn resume_claim_fixture(name: &str, submission_key: &str) -> ResumeClaimFixture {
    let root = TestDirectory::new(name);
    let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000001");
    let successor_run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000002");
    let backend = backend(root.path()).with_ready_timeout(Duration::ZERO);
    backend.create_run_storage(&run_id).assert_value();
    let submission = contract_submission(submission_key);
    backend
        .write_recovery_document(
            &run_id,
            &LocalRecoveryDocument {
                submission: submission.clone(),
                workspace: root.child("workspace"),
                delivery_run_id: Some(run_id.clone()),
                resumed_from: None,
                successor_run_id: Some(successor_run_id.clone()),
            },
        )
        .assert_value();
    let successor_storage = backend.create_run_storage(&successor_run_id).assert_value();
    ResumeClaimFixture {
        _root: root,
        backend,
        run_id,
        successor_run_id,
        submission,
        successor_storage,
    }
}

#[test]
fn interrupted_resume_releases_the_workspace_claim_for_retry() {
    let root = TestDirectory::new("lr-claim");
    let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000001");
    std::fs::create_dir_all(root.child("runs").join(run_id.as_str())).assert_value();
    let backend = backend(root.path());

    let interrupted_process_claim = backend.claim_recovery_workspace(&run_id).assert_value();
    assert!(
        !backend
            .recovery_workspace_is_unclaimed(&run_id)
            .assert_value()
    );

    drop(interrupted_process_claim);
    assert!(
        backend
            .recovery_workspace_is_unclaimed(&run_id)
            .assert_value()
    );
}

#[test]
fn stale_private_bootstraps_are_scavenged_without_touching_other_state() {
    let root = TestDirectory::new("stale-bootstrap");
    let run_id = RunId::new("0199f33f-3b44-7d21-9000-000000000001");
    let backend = backend(root.path());
    let storage = backend.create_run_storage(&run_id).assert_value();
    let bootstrap = storage.join(BOOTSTRAP_FILE);
    let retained = storage.join("retained-state");
    std::fs::write(&bootstrap, b"ambient-secret").assert_value();
    std::fs::write(&retained, b"keep").assert_value();

    backend
        .remove_stale_bootstraps(Duration::ZERO)
        .assert_value();

    assert!(!bootstrap.exists());
    assert!(retained.exists());
}

#[tokio::test]
async fn interrupted_resume_with_empty_ledger_clears_the_successor_claim() {
    let fixture = resume_claim_fixture("lre", "interrupted-resume");
    drop(SqliteRunLedger::open(fixture.successor_storage.join("runs.sqlite3")).assert_value());

    fixture
        .backend
        .reconcile_local_resume_claim(&fixture.run_id)
        .await
        .assert_value();

    assert!(!fixture.successor_storage.exists());
    assert_eq!(
        fixture
            .backend
            .read_recovery_document(&fixture.run_id)
            .assert_value()
            .successor_run_id,
        None
    );
}

#[tokio::test]
async fn interrupted_resume_with_a_durable_successor_keeps_the_claim() {
    let fixture = resume_claim_fixture("lrd", "durable-successor");
    let ledger =
        SqliteRunLedger::open(fixture.successor_storage.join("runs.sqlite3")).assert_value();
    let admitted = NativeV2Admission
        .admit(fixture.submission.clone())
        .await
        .assert_value();
    let digest = submission_digest(&fixture.submission).assert_value();
    ledger
        .create_or_get(CreateRun {
            run_id: fixture.successor_run_id.clone(),
            submission_key: fixture.submission.submission_key.clone(),
            submission_digest: digest,
            admitted,
        })
        .await
        .assert_value();
    drop(ledger);

    fixture
        .backend
        .reconcile_local_resume_claim(&fixture.run_id)
        .await
        .assert_value();

    assert!(fixture.successor_storage.exists());
    assert_eq!(
        fixture
            .backend
            .read_recovery_document(&fixture.run_id)
            .assert_value()
            .successor_run_id,
        Some(fixture.successor_run_id)
    );
}
