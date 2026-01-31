use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use zeroshot_tui::backend::stdio::StdioBackendClient;
use zeroshot_tui::backend::{BackendClient, PROTOCOL_VERSION};
use zeroshot_tui::protocol;

#[test]
fn stdio_backend_initialize_and_list_clusters() {
    std::env::set_var("ZEROSHOT_TUI_BACKEND_MOCK_LAUNCH", "1");
    std::env::set_var("ZEROSHOT_TUI_BACKEND_MOCK_GUIDANCE", "1");

    let backend_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mock_tui_backend.js");
    std::env::set_var("ZEROSHOT_TUI_BACKEND_PATH", backend_path);

    let params = protocol::InitializeParams {
        protocol_version: PROTOCOL_VERSION,
        client: protocol::ClientInfo {
            name: "zeroshot-tui-test".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: Some(std::process::id() as i64),
        },
        capabilities: Some(protocol::ClientCapabilities {
            wants_metrics: Some(true),
            wants_topology: Some(false),
        }),
    };

    let client = StdioBackendClient::connect(params).expect("connect backend");
    let clusters = client.list_clusters().expect("list clusters");
    assert!(clusters.clusters.is_empty());

    let mut got_notification = false;
    for _ in 0..10 {
        if let Some(notification) = client.try_next_notification().expect("notify") {
            match notification {
                zeroshot_tui::backend::BackendNotification::ClusterLogLines(_) => {
                    got_notification = true;
                    break;
                }
                _ => {}
            }
        }
        sleep(Duration::from_millis(10));
    }
    assert!(got_notification, "expected clusterLogLines notification");
}
