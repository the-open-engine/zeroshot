use openengine_cluster_testkit::artifacts::check_artifacts;
use std::path::PathBuf;

#[tokio::test]
async fn generated_artifacts_are_current() {
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    check_artifacts(&repository_root).await.unwrap();
}
