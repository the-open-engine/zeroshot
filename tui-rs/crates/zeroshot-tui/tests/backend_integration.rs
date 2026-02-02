use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use zeroshot_tui::backend::stdio::StdioBackendClient;
use zeroshot_tui::backend::{BackendClient, BackendConfig, BackendEvent};
use zeroshot_tui::protocol::{
    GetClusterSummaryParams, SubscribeClusterLogsParams, SubscribeClusterTimelineParams,
    UnsubscribeParams,
};

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static BACKEND_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

fn is_ci() -> bool {
    match std::env::var("CI") {
        Ok(value) => matches!(value.to_lowercase().as_str(), "1" | "true" | "yes"),
        Err(_) => false,
    }
}

struct EnvGuard {
    keys: Vec<&'static str>,
}

impl EnvGuard {
    fn set(pairs: &[(&'static str, &'static str)]) -> Self {
        let mut keys = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            std::env::set_var(key, value);
            keys.push(*key);
        }
        Self { keys }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for key in &self.keys {
            std::env::remove_var(key);
        }
    }
}

fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

fn repo_root() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    for ancestor in cwd.ancestors() {
        if ancestor.join("package.json").is_file() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn find_backend_path(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        let candidate = ancestor.join("lib/tui-backend/server.js");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn resolve_backend_path() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    if let Some(path) = find_backend_path(&cwd) {
        return Some(path);
    }

    let root = repo_root()?;
    let status = Command::new("npm")
        .args(["run", "build:tui-backend"])
        .current_dir(&root)
        .status()
        .ok()?;
    if !status.success() {
        eprintln!("npm run build:tui-backend failed");
        return None;
    }

    find_backend_path(&root)
}

fn build_client() -> Option<StdioBackendClient> {
    let backend_path = BACKEND_PATH.get_or_init(resolve_backend_path).clone();
    let Some(backend_path) = backend_path else {
        if is_ci() {
            panic!("TUI backend not available. Run `npm ci` and `npm run build:tui-backend` before cargo test.");
        }
        return None;
    };
    let mut config = BackendConfig::with_backend_path(backend_path);
    config.request_timeout = Some(Duration::from_secs(10));
    match StdioBackendClient::connect(config) {
        Ok(client) => Some(client),
        Err(err) => {
            if is_ci() {
                panic!("Failed to connect to TUI backend: {err}");
            }
            eprintln!("Skipping backend integration: {err}");
            None
        }
    }
}

#[test]
fn initialize_and_list_clusters() {
    let _guard = env_lock();
    let _env = EnvGuard::set(&[
        ("ZEROSHOT_TUI_BACKEND_MOCK_LAUNCH", "1"),
        ("ZEROSHOT_TUI_BACKEND_MOCK_GUIDANCE", "1"),
    ]);

    let Some(client) = build_client() else {
        eprintln!("Skipping backend integration: backend build unavailable");
        return;
    };
    assert_eq!(client.protocol_version(), 1);
    assert!(client.server_capabilities().is_some());
    let result = client.list_clusters().expect("listClusters");
    let _ = result.clusters.len();
}

#[test]
fn interleaved_notifications_do_not_break_requests() {
    let _guard = env_lock();
    let _env = EnvGuard::set(&[
        ("ZEROSHOT_TUI_BACKEND_MOCK_LAUNCH", "1"),
        ("ZEROSHOT_TUI_BACKEND_MOCK_GUIDANCE", "1"),
    ]);

    let Some(mut client) = build_client() else {
        eprintln!("Skipping backend integration: backend build unavailable");
        return;
    };
    let events = client.take_event_receiver().expect("event receiver");

    let subscription = client
        .subscribe_cluster_logs(SubscribeClusterLogsParams {
            cluster_id: "unknown-cluster".to_string(),
            agent_id: None,
        })
        .expect("subscribe logs");

    let _timeline = client
        .subscribe_cluster_timeline(SubscribeClusterTimelineParams {
            cluster_id: "unknown-cluster".to_string(),
        })
        .expect("subscribe timeline");

    let list_result = client.list_clusters().expect("listClusters");
    let summary = list_result
        .clusters
        .get(0)
        .map(|cluster| cluster.id.clone())
        .unwrap_or_else(|| "unknown-cluster".to_string());
    let _ = client
        .get_cluster_summary(GetClusterSummaryParams {
            cluster_id: summary,
        })
        .err();

    let _ = events.recv_timeout(Duration::from_millis(200));

    let _ = client
        .unsubscribe(UnsubscribeParams {
            subscription_id: subscription.subscription_id,
        })
        .expect("unsubscribe");
}

#[test]
fn backend_exit_and_drop() {
    let _guard = env_lock();
    let _env = EnvGuard::set(&[
        ("ZEROSHOT_TUI_BACKEND_MOCK_LAUNCH", "1"),
        ("ZEROSHOT_TUI_BACKEND_MOCK_GUIDANCE", "1"),
    ]);

    let Some(mut client) = build_client() else {
        eprintln!("Skipping backend integration: backend build unavailable");
        return;
    };
    let events = client.take_event_receiver().expect("event receiver");

    client.shutdown().expect("shutdown");
    let event = events
        .recv_timeout(Duration::from_secs(2))
        .expect("backend exit event");
    match event {
        BackendEvent::BackendExited(_) => {}
        BackendEvent::Notification(_) => panic!("expected BackendExited"),
    }
}
