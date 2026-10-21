use openengine_cluster_protocol::{ConnectionScope, RunId, RunListParams, TargetOecpSessionRequest};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use zeroshot_engine::native_v2_cli::DelegationAction;
use super::*;

const API_KEY: &str = "test-machine-secret";
const ORGANIZATION: &str = "00000000-0000-4000-8000-000000000001";

#[tokio::test]
async fn machine_http_problem_echoes_are_redacted_before_cli_diagnostics() {
    use zeroshot_engine::native_v2_cli::{NativeV2CliBackend, oecp::NamedTargetCliBackend};
    let root = super::super::fixtures::temp_root();
    let (listener, _, origin) = bind_target_authority().await;
    let server_origin = origin.clone();
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.assert_value();
            let request = read_http_request(&mut stream).await;
            if request.path == "/.well-known/zeroshot-native-v2" {
                let body = hosted_discovery(&server_origin);
                write_http_response(&mut stream, &body).await;
            } else {
                assert_eq!(request.path, "/native-v2/runs");
                assert_eq!(
                    request.authorization.as_deref(),
                    Some(format!("Bearer {API_KEY}").as_str())
                );
                let problem = json!({
                    "code": format!("unauthorized_{API_KEY}"),
                    "message": format!("Invalid credential {API_KEY}"),
                    "details": {API_KEY: [{"echo": API_KEY}], "retained": "safe detail"}
                });
                write_http_response_with_status(
                    &mut stream,
                    "401 Unauthorized",
                    &problem.to_string(),
                )
                .await;
            }
        }
    });
    let target = super::super::fixtures::hosted_target("prod", origin);
    let registry = super::super::fixtures::MemoryRegistry::default();
    registry.insert(target).assert_value();
    let authority = super::super::fixtures::test_http_authority(root.path("refresh-locks"))
        .with_machine_key(API_KEY, None);
    let backend = NamedTargetCliBackend::new(NativeV2TargetConnector::new(
        registry,
        authority,
        super::super::fixtures::FakeDialer::default(),
    ));
    let error = backend
        .run_list(Some("prod"), RunListParams {})
        .await
        .assert_error();
    assert!(!format!("{error:?}").contains(API_KEY));
    assert!(!error.to_string().contains(API_KEY));
    let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();
    assert!(!diagnostic.to_string().contains(API_KEY));
    assert_eq!(diagnostic["details"]["httpStatus"], 401);
    assert_eq!(diagnostic["details"]["retained"], "safe detail");
    assert_eq!(diagnostic["details"]["[REDACTED]"][0]["echo"], "[REDACTED]");
    server.await.assert_value();
}

#[tokio::test]
async fn machine_auth_uses_key_for_all_target_transports_without_oauth_or_keyring() {
    let root = super::super::fixtures::temp_root();
    let (origin, server) = spawn_target_authority(13).await;
    let target = super::super::fixtures::hosted_target("prod", origin);
    let authority = super::super::fixtures::test_http_authority(root.path("refresh-locks"))
        .with_machine_key(API_KEY, Some(ORGANIZATION));
    authority.discover(&target).await.assert_value();
    assert_eq!(
        authority
            .default_resource_scope(&target)
            .await
            .assert_value(),
        ConnectionScope::Org
    );
    authority
        .connection_list(
            &target,
            ConnectionListRequest {
                scope: ConnectionScope::Org,
            },
        )
        .await
        .assert_value();
    authority
        .profile_list(
            &target,
            RunProfileListRequest {
                scope: openengine_cluster_protocol::RunProfileScope::Org,
            },
        )
        .await
        .assert_value();
    authority
        .hosted_run_list(&target, RunListParams {})
        .await
        .assert_value();
    authority
        .oecp_session(
            &target,
            &TargetOecpSessionRequest {
                run_id: Some(RunId::new("run-hosted")),
            },
        )
        .await
        .assert_value();
    authority
        .hosted_run_checkpoints(
            &target,
            openengine_cluster_protocol::RunCheckpointsParams {
                run_id: RunId::new("run-recoverable"),
                after: None,
                limit: None,
            },
        )
        .await
        .assert_value();
    let requests = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .assert_value()
        .assert_value();
    assert!(
        requests
            .iter()
            .all(|request| !request.path.starts_with("/oauth/"))
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.authorization.is_some())
            .count(),
        6
    );
    for request in requests
        .iter()
        .filter(|request| request.authorization.is_some())
    {
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer {API_KEY}").as_str())
        );
        assert_eq!(request.organization.as_deref(), Some(ORGANIZATION));
        assert!(!request.body.contains(API_KEY));
    }
}

#[tokio::test]
async fn machine_keys_cannot_manage_delegations_or_start_human_login() {
    let root = super::super::fixtures::temp_root();
    let target = super::super::fixtures::hosted_target("prod", "https://unused.example".to_owned());
    let authority = super::super::fixtures::test_http_authority(root.path("refresh-locks"))
        .with_machine_key(API_KEY, None);
    assert!(
        authority
            .login(&target)
            .await
            .assert_error()
            .to_string()
            .contains("human login required")
    );
    assert!(
        authority
            .delegation(
                &target,
                DelegationAction::List {
                    owner: openengine_cluster_protocol::DelegationOwner::User,
                    organization_id: None,
                }
            )
            .await
            .assert_error()
            .to_string()
            .contains("human login required")
    );
}

#[tokio::test]
async fn user_machine_keys_default_to_the_user_catalog_without_oauth() {
    let root = super::super::fixtures::temp_root();
    let (origin, server) = spawn_target_authority(2).await;
    let target = super::super::fixtures::hosted_target("prod", origin);
    let authority = super::super::fixtures::test_http_authority(root.path("refresh-locks"))
        .with_machine_key("user-machine-key", None);
    assert_eq!(
        authority
            .default_resource_scope(&target)
            .await
            .assert_value(),
        ConnectionScope::User
    );
    let requests = server.await.assert_value();
    assert_eq!(requests[1].path, "/api/v1/machine");
    assert_eq!(
        requests[1].authorization.as_deref(),
        Some("Bearer user-machine-key")
    );
    assert!(requests[1].organization.is_none());
}

#[tokio::test]
async fn machine_keys_reject_direct_targets_before_any_network_request() {
    let root = super::super::fixtures::temp_root();
    let target = super::super::fixtures::direct_target("http://127.0.0.1:1");
    let authority = super::super::fixtures::test_http_authority(root.path("refresh-locks"))
        .with_machine_key(API_KEY, None);
    assert_eq!(
        authority.discover(&target).await.assert_error().to_string(),
        "ZEROSHOT_API_KEY requires a hosted target"
    );
    assert_eq!(
        authority
            .default_resource_scope(&target)
            .await
            .assert_error()
            .to_string(),
        "ZEROSHOT_API_KEY requires a hosted target"
    );
}

#[tokio::test]
async fn human_cli_manages_delegations_with_account_admin_authority_and_one_time_keys() {
    use openengine_cluster_protocol::{
        DelegationCreateRequest, DelegationKeyCreateRequest, DelegationOwner,
    };
    use zeroshot_engine::native_v2_cli::DelegationResult;
    let root = super::super::fixtures::temp_root();
    let (origin, server) = spawn_delegation_authority(23).await;
    let target = super::super::fixtures::hosted_target("prod", origin);
    let (credentials, authority) = test_authority(&root);
    credentials
        .set(&target.id, "refresh-0")
        .await
        .assert_value();
    let created = authority
        .delegation(
            &target,
            DelegationAction::Create(DelegationCreateRequest {
                name: "CI".to_owned(),
                owner: DelegationOwner::User,
                organization_id: None,
                expires_in_days: 90,
            }),
        )
        .await
        .assert_value();
    assert!(matches!(&created, DelegationResult::Created(created) if created.api_key == API_KEY));
    assert!(!format!("{created:?}").contains(API_KEY));
    let actions = [
        DelegationAction::List {
            owner: DelegationOwner::User,
            organization_id: None,
        },
        DelegationAction::Show {
            id: ORGANIZATION.to_owned(),
        },
        DelegationAction::KeyCreate {
            id: ORGANIZATION.to_owned(),
            request: DelegationKeyCreateRequest {
                name: Some("rotation".to_owned()),
                expires_in_days: 30,
            },
        },
        DelegationAction::KeyList {
            id: ORGANIZATION.to_owned(),
        },
        DelegationAction::KeyRevoke {
            id: ORGANIZATION.to_owned(),
            key_id: ORGANIZATION.to_owned(),
        },
        DelegationAction::Disable {
            id: ORGANIZATION.to_owned(),
        },
    ];
    for action in actions {
        authority.delegation(&target, action).await.assert_value();
    }
    let requests = tokio::time::timeout(std::time::Duration::from_secs(5), server)
        .await
        .assert_value()
        .assert_value();
    let token_requests = requests
        .iter()
        .filter(|request| request.path == "/oauth/token")
        .collect::<Vec<_>>();
    assert_eq!(token_requests.len(), 1);
    assert!(token_requests[0].body.contains("audience=admin"));
    assert!(requests.iter().all(|request| request.path != "/session"));
    assert!(
        requests
            .iter()
            .filter(|request| request.path.starts_with("/auth/v1/delegations"))
            .all(
                |request| request.authorization.as_deref() == Some("Bearer access-1")
                    && request.organization.is_none()
            )
    );
}

async fn spawn_delegation_authority(
    request_count: usize,
) -> (String, tokio::task::JoinHandle<Vec<CapturedHttpRequest>>) {
    let (listener, address, origin) = bind_target_authority().await;
    let server_origin = origin.clone();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut token_index = 0;
        for _ in 0..request_count {
            let (mut stream, _) = listener.accept().await.assert_value();
            let request = read_http_request(&mut stream).await;
            let (status, body) = delegation_response(&request).unwrap_or_else(|| {
                (
                    "200 OK",
                    authority_response(&request, &server_origin, address, &mut token_index),
                )
            });
            write_http_response_with_status(&mut stream, status, &body).await;
            requests.push(request);
        }
        requests
    });
    (origin, task)
}

fn delegation_response(request: &CapturedHttpRequest) -> Option<(&'static str, String)> {
    let suffix = request.path.strip_prefix("/auth/v1/delegations")?;
    let key = json!({
        "id": ORGANIZATION, "name": "API key", "prefix": "zsk_public", "created_at": "2026-01-01T00:00:00Z",
        "expires_at": "2026-04-01T00:00:00Z", "last_used_at": null, "revoked_at": null,
    });
    let delegation = json!({
        "id": ORGANIZATION, "name": "CI", "owner":"user", "user_id":ORGANIZATION, "organization_id":null,
        "created_at":"2026-01-01T00:00:00Z", "disabled_at":null, "keys":[key.clone()],
    });
    let value = match request.method.as_str() {
        "DELETE" => return Some(("204 No Content", String::new())),
        "POST" if suffix.ends_with("/keys") => json!({"key":key,"api_key":API_KEY}),
        "POST" => json!({"delegation":delegation,"key":key,"api_key":API_KEY}),
        "GET" => delegation_read_response(suffix, key, delegation),
        _ => return None,
    };
    Some(("200 OK", value.to_string()))
}

fn delegation_read_response(
    suffix: &str,
    key: serde_json::Value,
    delegation: serde_json::Value,
) -> serde_json::Value {
    if suffix.ends_with("/keys") {
        json!({"keys":[key]})
    } else if suffix.starts_with('?') {
        json!({"delegations":[delegation]})
    } else {
        delegation
    }
}
