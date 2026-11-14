use super::*;
use openengine_cluster_protocol::{RunVectorParams, RunVectorsParams, VectorId};

pub(super) async fn assert_vectors(fixture: &Fixture) {
    let page = fixture
        .backend
        .run_vectors(
            None,
            RunVectorsParams {
                run_id: fixture.id.clone(),
                after: None,
                limit: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        page.vectors.as_slice(),
        std::slice::from_ref(&fixture.vector.clone().into())
    );
    assert!(page.next_after.is_none());
    let vector = fixture
        .backend
        .run_vector(
            None,
            RunVectorParams {
                run_id: fixture.id.clone(),
                id: fixture.vector.id.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(vector, fixture.vector);
    let missing = VectorId::new("unknown").unwrap();
    let errors = [
        fixture
            .backend
            .run_vector(
                None,
                RunVectorParams {
                    run_id: fixture.id.clone(),
                    id: missing.clone(),
                },
            )
            .await
            .unwrap_err(),
        fixture
            .backend
            .run_vectors(
                None,
                RunVectorsParams {
                    run_id: fixture.id.clone(),
                    after: Some(missing),
                    limit: None,
                },
            )
            .await
            .unwrap_err(),
    ];
    for error in errors {
        let diagnostic = serde_json::to_value(error.diagnostic()).unwrap();
        assert_eq!(diagnostic["kind"], "run_not_found");
        assert_eq!(diagnostic["code"], "run.not_found");
        assert_eq!(diagnostic["message"], "vector was not found");
    }
}

async fn assert_boundary_refused(fixture: &Fixture) {
    let expected = fixture
        .backend
        .status_local(RunStatusParams {
            run_id: fixture.id.clone(),
        })
        .await
        .unwrap_err()
        .to_string();
    let page = fixture
        .backend
        .run_vectors(
            None,
            RunVectorsParams {
                run_id: fixture.id.clone(),
                after: None,
                limit: None,
            },
        )
        .await;
    let vector = fixture
        .backend
        .run_vector(
            None,
            RunVectorParams {
                run_id: fixture.id.clone(),
                id: fixture.vector.id.clone(),
            },
        )
        .await;
    assert_eq!(
        (
            page.err().map(|e| e.to_string()),
            vector.err().map(|e| e.to_string())
        ),
        (Some(expected.clone()), Some(expected))
    );
}

#[tokio::test]
async fn vector_reads_refuse_redirected_run_directories() {
    let fixture = Fixture::new(true, false).await;
    assert_vectors(&fixture).await;
    let paths = fixture.backend.paths(&fixture.id).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let redirected = outside.path().join("run");
    std::fs::rename(paths.storage(), &redirected).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&redirected, paths.storage()).unwrap();
    #[cfg(windows)]
    assert!(
        Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(paths.storage())
            .arg(&redirected)
            .output()
            .unwrap()
            .status
            .success()
    );
    let before = tree(outside.path());
    assert_boundary_refused(&fixture).await;
    assert_eq!(before, tree(outside.path()));
    #[cfg(unix)]
    std::fs::remove_file(paths.storage()).unwrap();
    #[cfg(windows)]
    std::fs::remove_dir(paths.storage()).unwrap();
}

#[tokio::test]
async fn vector_reads_refuse_redirected_ledgers() {
    let fixture = Fixture::new(true, false).await;
    assert_vectors(&fixture).await;
    let paths = fixture.backend.paths(&fixture.id).unwrap();
    let outside = tempfile::tempdir().unwrap();
    let redirected = outside.path().join("runs.sqlite3");
    std::fs::rename(paths.ledger(), &redirected).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&redirected, paths.ledger()).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&redirected, paths.ledger()).unwrap();
    let before = tree(outside.path());
    assert_boundary_refused(&fixture).await;
    assert_eq!(before, tree(outside.path()));
}
