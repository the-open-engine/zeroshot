use openengine_cluster_testkit::artifacts::{check_artifacts, workspace_root, write_artifacts};

#[tokio::main]
async fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("--write") => write_artifacts(&workspace_root()).await,
        Some("--check") => check_artifacts(&workspace_root()).await,
        _ => {
            eprintln!("usage: generate-cluster-protocol (--write|--check)");
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
