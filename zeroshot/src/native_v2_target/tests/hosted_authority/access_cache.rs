use std::sync::Arc;

use openengine_cluster_protocol::{
    ConnectionListRequest, ConnectionScope, RunId, RunListParams, RunStatusParams,
    RunProfileListRequest, RunProfileScope, TargetOecpSessionRequest,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
#[cfg(feature = "ui")]
use zeroshot_engine::profile_ui::{RunHistoryRequest, RunHistoryTransport, RunHistoryTransportError};

use super::super::fixtures::{hosted_target, temp_root, TempRoot};
#[cfg(feature = "ui")]
use super::super::fixtures::direct_target;
use super::*;
use crate::native_v2_target::controller_authority::credentials::test_support::MemoryDeviceCodeNotifier;

#[tokio::test]
async fn mixed_hosted_operations_reuse_one_validated_persisted_token() {
    let root = temp_root();
    let (origin, server) = spawn_target_authority(17).await;
    let credentials = Arc::new(RotatingCredentialStore::new("refresh-0"));
    let authority = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("refresh-locks"),
    );
    let target = hosted_target("local", origin);
    authority
        .hosted_run_status(
            &target,
            RunStatusParams {
                run_id: RunId::new("run-hosted"),
            },
        )
        .await
        .assert_value();
    authority
        .clone()
        .oecp_session(
            &target,
            &TargetOecpSessionRequest {
                run_id: Some(RunId::new("run-hosted")),
            },
        )
        .await
        .assert_value();
    authority
        .connection_list(
            &target,
            ConnectionListRequest {
                scope: ConnectionScope::User,
            },
        )
        .await
        .assert_value();
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    authority
        .profile_list(
            &target,
            RunProfileListRequest {
                scope: RunProfileScope::User,
            },
        )
        .await
        .assert_value();

    assert_eq!(credentials.value(), "refresh-1");
    let requests = server.await.assert_value();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/oauth/token")
            .count(),
        1
    );
    assert!(
        requests
            .iter()
            .filter_map(|request| request.authorization.as_deref())
            .all(|authorization| authorization == "Bearer access-1")
    );
}

#[tokio::test]
#[cfg(feature = "ui")]
async fn hosted_history_uses_discovered_routes_and_the_shared_validated_token() {
    let root = temp_root();
    let (origin, server) = spawn_target_authority(6).await;
    let transport = history_transport(&root, origin).await;
    let id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
    transport
        .get(RunHistoryRequest::list(None))
        .await
        .assert_value();
    transport
        .get(RunHistoryRequest::detail(id.clone()))
        .await
        .assert_value();

    let requests = server.await.assert_value();
    let history = requests
        .iter()
        .filter(|request| request.path.starts_with("/native-v2/workspaces/user/runs"))
        .collect::<Vec<_>>();
    assert_eq!(
        history
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        [
            "/native-v2/workspaces/user/runs",
            "/native-v2/workspaces/user/runs/018f5e78-7f95-7c22-8d98-3f15af20c991"
        ]
    );
    assert!(history.iter().all(|request| {
        request.method == "GET" && request.authorization.as_deref() == Some("Bearer access-1")
    }));
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/oauth/token")
            .count(),
        1
    );
    for path in ["/.well-known/zeroshot-native-v2", "/oauth/metadata"] {
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path == path)
                .count(),
            1,
            "repeated {path}"
        );
    }
}

#[tokio::test]
#[cfg(feature = "ui")]
async fn hosted_history_auth_rejection_invalidates_without_replaying_the_read() {
    let root = temp_root();
    let (origin, server) = spawn_custom_authority(
        8,
        reject_first("/native-v2/workspaces/user/runs", "401 Unauthorized"),
    )
    .await;
    let transport = history_transport(&root, origin).await;
    transport
        .get(RunHistoryRequest::list(None))
        .await
        .assert_value();
    transport
        .get(RunHistoryRequest::list(None))
        .await
        .assert_value();

    let requests = server.await.assert_value();
    let history = requests
        .iter()
        .filter(|request| request.path == "/native-v2/workspaces/user/runs")
        .collect::<Vec<_>>();
    assert_eq!(history.len(), 2);
    assert_eq!(
        history
            .iter()
            .filter_map(|request| request.authorization.as_deref())
            .collect::<Vec<_>>(),
        ["Bearer access-1", "Bearer access-2"]
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/oauth/token")
            .count(),
        2
    );
}

#[tokio::test]
#[cfg(feature = "ui")]
async fn direct_history_uses_discovered_routes_without_authorization() {
    let root = temp_root();
    let (origin, server) = spawn_direct_target_authority(3).await;
    let (_, authority) = test_authority(&root);
    let transport = TargetRunHistoryTransport::new(authority, direct_target(origin)).assert_value();
    let id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
    transport
        .get(RunHistoryRequest::list(Some(id.clone())))
        .await
        .assert_value();
    transport
        .get(RunHistoryRequest::detail(id.clone()))
        .await
        .assert_value();

    let requests = server.await.assert_value();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.path.as_str())
            .collect::<Vec<_>>(),
        [
            "/.well-known/zeroshot-native-v2",
            "/direct-history?after=018f5e78-7f95-7c22-8d98-3f15af20c991",
            "/direct-history/018f5e78-7f95-7c22-8d98-3f15af20c991"
        ]
    );
    assert!(
        requests
            .iter()
            .all(|request| request.authorization.is_none())
    );
}

#[tokio::test]
#[cfg(feature = "ui")]
async fn history_rejects_absent_or_unsafe_discovered_capabilities_explicitly() {
    for case in 0..4 {
        let root = temp_root();
        let (origin, server) = spawn_custom_authority(1, move |request, body| {
            if request.path != "/.well-known/zeroshot-native-v2" {
                return ("200 OK", body);
            }
            let mut discovery: serde_json::Value = serde_json::from_str(&body).assert_value();
            match case {
                0 => {
                    discovery["extensions"]
                        .as_object_mut()
                        .assert_value()
                        .remove("run_history");
                }
                1 => {
                    discovery["extensions"]["run_history"]["kind"] =
                        json!("zeroshot.run-history/v2")
                }
                2 => {
                    discovery["extensions"]["run_history"]["baseUrl"] =
                        json!("https://attacker.example")
                }
                _ => {
                    discovery["extensions"]["run_history"]["routeTemplates"]["list"] =
                        json!("/../runs{?after}")
                }
            }
            ("200 OK", discovery.to_string())
        })
        .await;
        let (_, authority) = test_authority(&root);
        let transport = TargetRunHistoryTransport::new(authority, hosted_target("cloud", origin))
            .assert_value();
        assert_eq!(
            transport
                .get(RunHistoryRequest::list(None))
                .await
                .err()
                .assert_value(),
            RunHistoryTransportError::Incompatible
        );
        server.await.assert_value();
    }
}

#[cfg(feature = "ui")]
async fn history_transport(root: &TempRoot, origin: String) -> TargetRunHistoryTransport {
    let (credentials, authority) = test_authority(root);
    let target = hosted_target("cloud", origin);
    credentials
        .set(&target.id, "refresh-0")
        .await
        .assert_value();
    TargetRunHistoryTransport::new(authority, target).assert_value()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authority_clone_misses_coalesce_before_reading_refresh_credentials() {
    let root = temp_root();
    let (origin, server) = spawn_target_authority(8).await;
    let credentials = Arc::new(RotatingCredentialStore::new("refresh-0"));
    let authority = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("refresh-locks"),
    );
    let target = hosted_target("local", origin);
    let clone = authority.clone();
    let (first, second) = tokio::join!(
        authority.hosted_run_list(&target, RunListParams {}),
        clone.oecp_session(&target, &TargetOecpSessionRequest { run_id: None }),
    );
    first.assert_value();
    second.assert_value();
    assert_eq!(credentials.value(), "refresh-1");
    let requests = server.await.assert_value();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/oauth/token")
            .count(),
        1
    );
}

#[tokio::test]
async fn cancelled_access_waiter_cannot_cancel_refresh_rotation() {
    let root = temp_root();
    let (origin, server) = spawn_target_authority(7).await;
    let credentials = Arc::new(PersistenceBlockingCredentialStore::new("refresh-0"));
    let authority = authority_with_credentials(&root, credentials.clone());
    let target = hosted_target("local", origin);
    let cancelled_authority = authority.clone();
    let cancelled_target = target.clone();
    let cancelled = tokio::spawn(async move {
        cancelled_authority
            .hosted_run_list(&cancelled_target, RunListParams {})
            .await
    });
    credentials.wait_until_persisting().await;
    cancelled.abort();
    assert!(cancelled.await.assert_error().is_cancelled());
    credentials.release_persistence();
    credentials.wait_until_persisted().await;

    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    assert_eq!(credentials.value(), "refresh-1");
    let requests = server.await.assert_value();
    assert_eq!(request_count(&requests, "/oauth/token"), 1);
    assert_eq!(request_count(&requests, "/native-v2/runs"), 1);
}

#[tokio::test]
async fn short_lived_access_tokens_are_never_reused() {
    for expires_in in [1, 29, 30, 3600] {
        let root = temp_root();
        let exchanges = if expires_in > 30 { 1 } else { 2 };
        let (origin, server) = spawn_custom_authority(6 + 2 * exchanges, move |request, body| {
            if request.path == "/oauth/token" {
                let mut token: serde_json::Value = serde_json::from_str(&body).assert_value();
                token["expires_in"] = json!(expires_in);
                ("200 OK", token.to_string())
            } else {
                ("200 OK", body)
            }
        })
        .await;
        let (credentials, authority) = test_authority(&root);
        let target = hosted_target("local", origin);
        credentials
            .set(&target.id, "refresh-0")
            .await
            .assert_value();
        for _ in 0..2 {
            authority
                .hosted_run_list(&target, RunListParams {})
                .await
                .assert_value();
        }
        let requests = server.await.assert_value();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path == "/oauth/token")
                .count(),
            exchanges
        );
    }
}

#[tokio::test]
async fn hosted_auth_rejection_invalidates_without_replaying_the_operation() {
    assert_rejection_cache_effect(13, "401 Unauthorized", 2, "Bearer access-2").await;
}

#[tokio::test]
async fn hosted_resource_forbidden_preserves_the_cached_issuance() {
    assert_rejection_cache_effect(11, "403 Forbidden", 1, "Bearer access-1").await;
}

async fn assert_rejection_cache_effect(
    request_count: usize,
    rejection: &'static str,
    expected_token_requests: usize,
    expected_authorization: &str,
) {
    let root = temp_root();
    let (origin, server) = spawn_custom_authority(
        request_count,
        reject_first("/native-v2/oecp-session", rejection),
    )
    .await;
    let (credentials, authority) = test_authority(&root);
    let target = hosted_target("local", origin);
    credentials
        .set(&target.id, "refresh-0")
        .await
        .assert_value();
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    authority
        .oecp_session(&target, &TargetOecpSessionRequest { run_id: None })
        .await
        .assert_error();
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    let requests = server.await.assert_value();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/native-v2/oecp-session")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/oauth/token")
            .count(),
        expected_token_requests
    );
    assert_eq!(
        requests.last().assert_value().authorization.as_deref(),
        Some(expected_authorization)
    );
}

fn reject_first(
    path: &'static str,
    status: &'static str,
) -> impl FnMut(&CapturedHttpRequest, String) -> (&'static str, String) + Send + 'static {
    let mut rejected = false;
    move |request, body| {
        if request.path == path && !rejected {
            rejected = true;
            (
                status,
                json!({"code": "unauthorized", "message": "revoked"}).to_string(),
            )
        } else {
            ("200 OK", body)
        }
    }
}

#[tokio::test]
async fn access_cache_isolates_target_login_and_origin_and_keeps_only_one_context() {
    for change in ["target", "login", "origin"] {
        let root = temp_root();
        let count = if change == "origin" { 10 } else { 15 };
        let (origin, server) = spawn_target_authority(count).await;
        let (credentials, authority) = test_authority(&root);
        let target = hosted_target("local", origin);
        let mut other = target.clone();
        let mut other_server = None;
        match change {
            "target" => other.id = "33333333-3333-4333-8333-333333333333".to_owned(),
            "login" => {
                other.access = TargetAccess::Hosted {
                    device_token: "44444444-4444-4444-8444-444444444444".to_owned(),
                }
            }
            _ => {
                let (origin, server) = spawn_target_authority(5).await;
                other.origin = origin;
                other_server = Some(server);
            }
        }
        credentials
            .set(&target.id, "refresh-0")
            .await
            .assert_value();
        credentials.set(&other.id, "refresh-0").await.assert_value();
        for context in [&target, &other, &target] {
            authority
                .hosted_run_list(context, RunListParams {})
                .await
                .assert_value();
        }
        let mut requests = server.await.assert_value();
        if let Some(server) = other_server {
            requests.extend(server.await.assert_value());
        }
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path == "/oauth/token")
                .count(),
            3
        );
    }
}

#[tokio::test]
async fn access_cache_isolates_discovered_client_and_rejects_incompatible_audience() {
    for field in ["/oauth/clientId", "/audience"] {
        let root = temp_root();
        let mut discovery_count = 0;
        let request_count = if field == "/audience" { 6 } else { 10 };
        let (origin, server) = spawn_custom_authority(request_count, move |request, body| {
            if request.path == "/.well-known/zeroshot-native-v2" {
                discovery_count += 1;
                let mut discovery: serde_json::Value = serde_json::from_str(&body).assert_value();
                if discovery_count == 2 {
                    *discovery.pointer_mut(field).assert_value() = json!("other-identity");
                }
                ("200 OK", discovery.to_string())
            } else {
                ("200 OK", body)
            }
        })
        .await;
        let (credentials, authority) = test_authority(&root);
        let target = hosted_target("local", origin);
        credentials
            .set(&target.id, "refresh-0")
            .await
            .assert_value();
        authority
            .hosted_run_list(&target, RunListParams {})
            .await
            .assert_value();
        let second = authority.hosted_run_list(&target, RunListParams {}).await;
        if field == "/audience" {
            assert_eq!(
                second.assert_error().to_string(),
                "native-v2 controller discovery is incompatible"
            );
        } else {
            second.assert_value();
        }
        let requests = server.await.assert_value();
        let exchanges = requests
            .iter()
            .filter(|request| request.path == "/oauth/token")
            .collect::<Vec<_>>();
        if field == "/audience" {
            assert_eq!(exchanges.len(), 1);
        } else {
            assert_eq!(exchanges.len(), 2);
            assert!(exchanges[1].body.contains("other-identity"));
        }
    }
}

#[tokio::test]
async fn reuse_stops_when_a_token_enters_the_expiry_safety_margin() {
    let root = temp_root();
    let (origin, server) = spawn_custom_authority(10, |request, body| {
        if request.path == "/oauth/token" {
            let mut token: serde_json::Value = serde_json::from_str(&body).assert_value();
            token["expires_in"] = json!(31);
            ("200 OK", token.to_string())
        } else {
            ("200 OK", body)
        }
    })
    .await;
    let (credentials, authority) = test_authority(&root);
    let target = hosted_target("local", origin);
    credentials
        .set(&target.id, "refresh-0")
        .await
        .assert_value();
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    let requests = server.await.assert_value();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.path == "/oauth/token")
            .count(),
        2
    );
}

#[tokio::test]
async fn cancelled_login_clears_previous_access_across_clones_before_preflight() {
    let root = temp_root();
    let (origin, server) = spawn_target_authority(12).await;
    let credentials = Arc::new(LoginBlockingCredentialStore::new("refresh-0"));
    let authority = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("refresh-locks"),
    );
    let target = hosted_target("local", origin);
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    let login_authority = authority.clone();
    let login_target = target.clone();
    let login = tokio::spawn(async move { login_authority.login(&login_target).await });
    credentials.wait_until_prepared().await;
    login.abort();
    assert!(login.await.assert_error().is_cancelled());
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    assert_eq!(credentials.reads(), 2);
    assert_eq!(credentials.value(), "refresh-2");
    let requests = server.await.assert_value();
    assert!(
        requests
            .iter()
            .all(|request| request.path != "/oauth/device")
    );
}

#[tokio::test]
async fn synthetic_relogin_does_not_leave_the_previous_cached_login_active() {
    let root = temp_root();
    let (origin, server) = spawn_target_authority(15).await;
    let (credentials, authority) = test_authority(&root);
    let target = hosted_target("local", origin);
    credentials
        .set(&target.id, "refresh-0")
        .await
        .assert_value();
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    authority.clone().login(&target).await.assert_value();
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    let requests = server.await.assert_value();
    let authorizations = requests
        .iter()
        .filter(|request| request.path == "/native-v2/runs")
        .filter_map(|request| request.authorization.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(authorizations, ["Bearer access-1", "Bearer access-3"]);
    assert_eq!(
        credentials.get(&target.id).await.assert_value().as_deref(),
        Some("refresh-3")
    );
}

#[tokio::test]
async fn failed_session_validation_neither_persists_nor_caches_the_token() {
    let root = temp_root();
    let mut rejected = false;
    let (origin, server) = spawn_custom_authority(9, move |request, body| {
        if request.path == "/session" && !rejected {
            rejected = true;
            (
                "403 Forbidden",
                json!({"code": "forbidden", "message": "revoked"}).to_string(),
            )
        } else {
            ("200 OK", body)
        }
    })
    .await;
    let credentials = Arc::new(LoginBlockingCredentialStore::new("refresh-0"));
    let authority = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("refresh-locks"),
    );
    let target = hosted_target("local", origin);
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_error();
    assert_eq!(credentials.value(), "refresh-0");
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    assert_eq!(credentials.reads(), 2);
    assert_eq!(credentials.value(), "refresh-2");
    server.await.assert_value();
}

#[tokio::test]
async fn failed_refresh_persistence_never_publishes_an_access_token() {
    let root = temp_root();
    let (origin, server) = spawn_target_authority(8).await;
    let credentials = Arc::new(FailingPersistenceStore {
        reads: AtomicUsize::new(0),
    });
    let authority = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("refresh-locks"),
    );
    let target = hosted_target("local", origin);
    for _ in 0..2 {
        assert_eq!(
            authority
                .hosted_run_list(&target, RunListParams {})
                .await
                .assert_error()
                .to_string(),
            "synthetic persistence failure"
        );
    }
    assert_eq!(credentials.reads.load(Ordering::SeqCst), 2);
    let requests = server.await.assert_value();
    assert!(
        requests
            .iter()
            .all(|request| request.path != "/native-v2/runs")
    );
}

struct FailingPersistenceStore {
    reads: AtomicUsize,
}

fn authority_with_credentials(
    root: &TempRoot,
    credentials: Arc<dyn TargetCredentialStore>,
) -> TargetHttpControlAuthority {
    TargetHttpControlAuthority::with_dependencies(
        credentials,
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("refresh-locks"),
    )
}

fn request_count(requests: &[CapturedHttpRequest], path: &str) -> usize {
    requests
        .iter()
        .filter(|request| request.path == path)
        .count()
}

struct PersistenceBlockingCredentialStore {
    value: Mutex<String>,
    persist_started: Notify,
    persist_release: Notify,
    persist_completed: Notify,
}

impl PersistenceBlockingCredentialStore {
    fn new(value: &str) -> Self {
        Self {
            value: Mutex::new(value.to_owned()),
            persist_started: Notify::new(),
            persist_release: Notify::new(),
            persist_completed: Notify::new(),
        }
    }

    async fn wait_until_persisting(&self) {
        self.persist_started.notified().await;
    }

    fn release_persistence(&self) {
        self.persist_release.notify_one();
    }

    async fn wait_until_persisted(&self) {
        self.persist_completed.notified().await;
    }

    fn value(&self) -> String {
        self.value.lock().assert_value().clone()
    }
}

#[async_trait]
impl TargetCredentialStore for PersistenceBlockingCredentialStore {
    async fn get(&self, _target_id: &str) -> Result<Option<String>, TargetAuthorityError> {
        Ok(Some(self.value()))
    }

    async fn set(&self, _target_id: &str, refresh_token: &str) -> Result<(), TargetAuthorityError> {
        self.persist_started.notify_one();
        self.persist_release.notified().await;
        *self.value.lock().assert_value() = refresh_token.to_owned();
        self.persist_completed.notify_one();
        Ok(())
    }
}

#[async_trait]
impl TargetCredentialStore for FailingPersistenceStore {
    async fn get(&self, _target_id: &str) -> Result<Option<String>, TargetAuthorityError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        Ok(Some("synthetic-refresh".to_owned()))
    }

    async fn set(
        &self,
        _target_id: &str,
        _refresh_token: &str,
    ) -> Result<(), TargetAuthorityError> {
        Err(TargetAuthorityError::new("synthetic persistence failure"))
    }
}

async fn spawn_custom_authority(
    request_count: usize,
    mut response: impl FnMut(&CapturedHttpRequest, String) -> (&'static str, String) + Send + 'static,
) -> (String, tokio::task::JoinHandle<Vec<CapturedHttpRequest>>) {
    let (listener, address, origin) = bind_target_authority().await;
    let server_origin = origin.clone();
    let server = tokio::spawn(async move {
        let mut captured = Vec::new();
        let mut token_index = 0_u8;
        for _ in 0..request_count {
            let (mut stream, _) = listener.accept().await.assert_value();
            let request = read_http_request(&mut stream).await;
            let body = authority_response(&request, &server_origin, address, &mut token_index);
            let (status, body) = response(&request, body);
            write_http_response_with_status(&mut stream, status, &body).await;
            captured.push(request);
        }
        captured
    });
    (origin, server)
}
