use anyhow::{bail, Context};
use openengine_cluster_testkit::artifacts::{check_artifacts, write_artifacts};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mode = std::env::args().nth(1);
    if std::env::args().nth(2).is_some() {
        bail!("usage: generate-cluster-protocol (--write|--check)");
    }
    let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .context("testkit must live two directories below repository root")?
        .to_owned();
    match mode.as_deref() {
        Some("--write") => write_artifacts(&repository_root).await,
        Some("--check") => check_artifacts(&repository_root).await,
        _ => bail!("usage: generate-cluster-protocol (--write|--check)"),
    }
}
