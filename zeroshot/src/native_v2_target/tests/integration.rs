use std::collections::BTreeMap;
use std::sync::Arc;

use openengine_cluster_protocol::{
    ConnectionKey, EnvironmentVariableName, RunConnectionRequirements, RunDiscardWorkspaceParams,
    RunCheckpointsParams, RunId, RunResumeParams,
};
use openengine_cluster_testkit::assertions::{AssertAt, AssertError, AssertValue};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use zeroshot_engine::native_v2_cli::oecp::NamedTargetCliBackend;
use zeroshot_engine::native_v2_cli::NativeV2CliBackend;

use super::super::controller_authority::credentials::test_support::{
    MemoryCredentialStore, MemoryDeviceCodeNotifier,
};
use super::super::controller_authority::TargetCredentialStore;
use super::super::*;
use super::fixtures::*;
use super::hosted_authority::*;

type ServerRequest = tokio_tungstenite::tungstenite::handshake::server::Request;
type ServerResponse = tokio_tungstenite::tungstenite::handshake::server::Response;

#[test]
fn file_registry_initializes_cloud_with_a_stable_hosted_identity() {
    let root = temp_root();
    let path = root.path("config/targets.json");
    let cloud = FileTargetRegistry::new(path.clone())
        .get("cloud")
        .assert_value();
    assert_eq!(cloud.name, "cloud");
    assert_eq!(cloud.origin, "https://api.cloud.zeroshot.sh");
    assert!(matches!(cloud.access, TargetAccess::Hosted { .. }));
    assert_eq!(
        FileTargetRegistry::new(path).get("cloud").assert_value(),
        cloud
    );
    assert!(matches!(
        FileTargetRegistry::new(root.path("other/targets.json")).get("unknown"),
        Err(TargetConnectorError::NotFound(_))
    ));
}

#[test]
fn file_registry_round_trips_named_targets_without_credentials() {
    let root = temp_root();
    let path = root.path("config/targets.json");
    let registry = FileTargetRegistry::new(path.clone());
    registry.insert(target()).assert_value();
    assert_eq!(registry.get("prod").assert_value(), target());
    assert!(matches!(
        registry.insert(target()),
        Err(TargetConnectorError::AlreadyExists(_))
    ));
    let stored = std::fs::read_to_string(path).assert_value();
    assert!(!stored.contains("accessToken"));
    assert!(!stored.contains("refreshToken"));
    assert!(!stored.contains("runtime"));
}

#[test]
fn file_registry_round_trips_direct_access_without_a_device_credential() {
    let root = temp_root();
    let path = root.path("config/targets.json");
    let registry = FileTargetRegistry::new(path.clone());
    let direct = TargetRecord {
        id: "33333333-3333-4333-8333-333333333333".to_owned(),
        name: "vm".to_owned(),
        origin: "http://127.0.0.1:8080".to_owned(),
        access: TargetAccess::Direct,
    };
    registry.insert(direct.clone()).assert_value();
    assert_eq!(registry.get("vm").assert_value(), direct);
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).assert_value()).assert_value();
    assert_eq!(
        stored["targets"]["vm"]["access"],
        serde_json::json!({"mode": "direct"})
    );
}

#[test]
fn file_registry_keeps_recovery_authorization_outside_target_records() {
    let root = temp_root();
    let path = root.path("config/targets.json");
    let registry = FileTargetRegistry::new(path.clone());
    registry
        .insert(direct_target("http://127.0.0.1:8080"))
        .assert_value();
    let run_id = run_request().run_id;
    let requirements = BTreeMap::from([(
        ConnectionKey::new("openai").assert_value(),
        vec![EnvironmentVariableName::new("OPENAI_API_KEY").assert_value()],
    )]);

    registry
        .record_recovery_authorization(
            "11111111-1111-4111-8111-111111111111",
            &run_id,
            &requirements,
        )
        .assert_value();
    let different = BTreeMap::from([(
        ConnectionKey::new("aws").assert_value(),
        vec![EnvironmentVariableName::new("AWS_SECRET_ACCESS_KEY").assert_value()],
    )]);
    assert!(matches!(
        registry.record_recovery_authorization(
            "11111111-1111-4111-8111-111111111111",
            &run_id,
            &different,
        ),
        Err(TargetConnectorError::RecoveryAuthorizationMismatch)
    ));
    let authorization_path = path
        .with_extension("recovery")
        .join("11111111-1111-4111-8111-111111111111")
        .join(format!("{}.json", run_id.as_str()));
    drop(registry);
    let restarted = FileTargetRegistry::new(path.clone());
    assert_eq!(
        restarted
            .recovery_authorization("11111111-1111-4111-8111-111111111111", &run_id)
            .assert_value(),
        requirements
    );
    assert!(matches!(
        restarted.recovery_authorization("33333333-3333-4333-8333-333333333333", &run_id),
        Err(TargetConnectorError::RecoveryAuthorizationUnavailable)
    ));
    assert!(
        !std::fs::read_to_string(path)
            .assert_value()
            .contains("OPENAI_API_KEY")
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&authorization_path)
                .assert_value()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    std::fs::write(&authorization_path, b"{}").assert_value();
    assert!(matches!(
        restarted.recovery_authorization("11111111-1111-4111-8111-111111111111", &run_id),
        Err(TargetConnectorError::RecoveryAuthorizationInvalid)
    ));
    restarted
        .remove_recovery_authorization("11111111-1111-4111-8111-111111111111", &run_id)
        .assert_value();
    assert!(matches!(
        restarted.recovery_authorization("11111111-1111-4111-8111-111111111111", &run_id),
        Err(TargetConnectorError::RecoveryAuthorizationUnavailable)
    ));
}

#[test]
fn file_registry_accepts_legacy_repository_fields_without_changing_target_identity() {
    let root = temp_root();
    let path = root.path("config/targets.json");
    std::fs::create_dir_all(path.parent().assert_value()).assert_value();
    let target = target();
    let mut stored_target = serde_json::to_value(&target).assert_value();
    stored_target["repository"] = serde_json::json!("old/wrong-repository");
    stored_target["defaultBranch"] = serde_json::json!("stale");
    std::fs::write(
        &path,
        serde_json::to_vec(&serde_json::json!({
            "version": 4,
            "targets": {"prod": stored_target}
        }))
        .assert_value(),
    )
    .assert_value();

    assert_eq!(
        FileTargetRegistry::new(path.clone())
            .get("prod")
            .assert_value(),
        target
    );
    let migrated = std::fs::read_to_string(path).assert_value();
    assert!(migrated.contains(r#""version": 5"#));
    assert!(!migrated.contains("repository"));
    assert!(!migrated.contains("defaultBranch"));
}

#[cfg(unix)]
#[test]
fn file_registry_is_private_on_creation() {
    use std::os::unix::fs::PermissionsExt;

    let root = temp_root();
    let path = root.path("config/targets.json");
    FileTargetRegistry::new(path.clone())
        .insert(target())
        .assert_value();
    assert_eq!(
        std::fs::metadata(path).assert_value().permissions().mode() & 0o777,
        0o600
    );
}

#[tokio::test]
async fn connector_preserves_add_login_and_target_scoped_connect() {
    let registry = MemoryRegistry::default();
    let authority = FakeAuthority::new("wss://target.example/oecp");
    let dialer = FakeDialer::default();
    let connector = NativeV2TargetConnector::new(registry, authority.clone(), dialer.clone());

    connector
        .add(TargetAdd {
            name: "prod".to_owned(),
            url: "https://target.example".to_owned(),
            direct: false,
        })
        .await
        .assert_value();
    connector.login("prod").await.assert_value();
    let receipt = connector.submit("prod", run_request()).await.assert_value();
    connector
        .connect("prod", Some(receipt.run_id.clone()))
        .await
        .assert_value();
    assert!(
        connector
            .connect_workspace_recovery("prod", receipt.run_id.clone())
            .await
            .assert_error()
            .to_string()
            .contains("target does not advertise workspace recovery")
    );
    assert_eq!(receipt.run_id, run_request().run_id);

    let calls = authority.calls();
    let added = match calls.assert_at(0) {
        AuthorityCall::Discover(added) => Some(added),
        _ => None,
    };
    let added = added.assert_value_with("expected discovery");
    assert_eq!(added.name, "prod");
    assert_eq!(added.origin, "https://target.example");
    assert_eq!(added.id.len(), 36);
    assert!(matches!(
        &added.access,
        TargetAccess::Hosted { device_token } if device_token.len() == 36
    ));
    assert!(matches!(calls.assert_at(1), AuthorityCall::Login(record) if record == added));
    let submitted = match calls.assert_at(2) {
        AuthorityCall::Submit(record, intent) => Some((record, intent.as_ref())),
        _ => None,
    };
    let (record, request) = submitted.assert_value_with("expected target submission");
    assert_eq!(record, added);
    assert_eq!(request.run_id, run_request().run_id);
    assert_eq!(
        request.submission.source.repository.as_str(),
        "open-engine/zeroshot"
    );
    assert_eq!(request.submission.source.branch.as_str(), "feature");
    assert!(matches!(
        calls.assert_at(3),
        AuthorityCall::Session(record, session)
            if record == added && session.run_id == Some(run_request().run_id)
    ));
    assert_eq!(
        dialer.sessions.lock().assert_value().as_slice(),
        &[(added.clone(), "wss://target.example/oecp".to_owned())]
    );
}

type RecoveryConnector = NativeV2TargetConnector<MemoryRegistry, FakeAuthority, FakeDialer>;

struct DirectRecoveryFixture {
    connector: RecoveryConnector,
    registry: MemoryRegistry,
    target: TargetRecord,
    run_id: RunId,
    key: ConnectionKey,
    field: EnvironmentVariableName,
    trusted: RunConnectionRequirements,
}

async fn direct_recovery_fixture() -> DirectRecoveryFixture {
    let registry = MemoryRegistry::default();
    let target = direct_target("http://127.0.0.1:8080");
    registry.insert(target.clone()).assert_value();
    let authoritative_run_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c994");
    let authority = FakeAuthority::with_receipt(
        "ws://127.0.0.1:8080/native-v2/oecp",
        authoritative_run_id.clone(),
    );
    let connector =
        NativeV2TargetConnector::new(registry.clone(), authority, FakeDialer::default());
    let mut request = run_request();
    request.intent.runtime = serde_json::from_value(serde_json::json!({
        "harness":"codex",
        "provider":"openai",
        "size":"medium",
        "nodes":{
            "worker":{
                "kind":"agent",
                "model":"gpt-5.6-sol",
                "connections":{"openai":["OPENAI_API_KEY"]}
            }
        }
    }))
    .assert_value();
    let requested_run_id = request.run_id.clone();
    let receipt = connector.submit("vm", request).await.assert_value();
    assert_eq!(receipt.run_id, authoritative_run_id);
    let run_id = receipt.run_id;
    let key = ConnectionKey::new("openai").assert_value();
    let field = EnvironmentVariableName::new("OPENAI_API_KEY").assert_value();
    let trusted = BTreeMap::from([(key.clone(), vec![field.clone()])]);

    assert_eq!(
        registry
            .recovery_authorization(&target.id, &requested_run_id)
            .assert_value(),
        trusted
    );
    DirectRecoveryFixture {
        connector,
        registry,
        target,
        run_id,
        key,
        field,
        trusted,
    }
}

#[tokio::test]
async fn direct_connector_binds_resume_fields_to_the_original_runtime() {
    let fixture = direct_recovery_fixture().await;
    assert_eq!(
        fixture
            .connector
            .authorize_workspace_recovery_requirements(
                "vm",
                &fixture.run_id,
                fixture.trusted.clone(),
            )
            .assert_value(),
        fixture.trusted
    );
    let selected = EnvironmentVariableName::new("AWS_SECRET_ACCESS_KEY").assert_value();
    let untrusted = BTreeMap::from([(fixture.key.clone(), vec![selected])]);
    assert!(
        fixture
            .connector
            .authorize_workspace_recovery_requirements("vm", &fixture.run_id, untrusted)
            .assert_error()
            .to_string()
            .contains("do not match the original run")
    );
}

#[tokio::test]
async fn direct_connector_constrains_resume_values_and_propagates_authorization() {
    let fixture = direct_recovery_fixture().await;
    let selected = EnvironmentVariableName::new("AWS_SECRET_ACCESS_KEY").assert_value();
    let successor_run_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c992");
    let untrusted_values = openengine_cluster_protocol::StaticConnectionValues::new(
        BTreeMap::from([(selected, "secret".to_owned())]),
    )
    .assert_value();
    let params = RunResumeParams {
        run_id: fixture.run_id.clone(),
        successor_run_id: successor_run_id.clone(),
        connections: BTreeMap::from([(fixture.key.clone(), untrusted_values)]),
        connection_resolver: None,
        github_token: None,
        from: None,
    };
    assert!(
        fixture
            .connector
            .prepare_workspace_recovery_resume("vm", &params)
            .assert_error()
            .to_string()
            .contains("do not match the original run")
    );
    assert_recovery_unavailable(&fixture, &successor_run_id);

    let resolver_successor = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c993");
    let resolver_params = RunResumeParams {
        run_id: fixture.run_id.clone(),
        successor_run_id: resolver_successor.clone(),
        connections: BTreeMap::new(),
        connection_resolver: Some(openengine_cluster_protocol::TargetConnectionResolver {
            endpoint: "https://resolver.example".to_owned(),
            bearer_token: "resolver-token".to_owned(),
            keys: vec![fixture.key.clone()],
            source_connection: None,
        }),
        github_token: None,
        from: None,
    };
    assert!(
        fixture
            .connector
            .prepare_workspace_recovery_resume("vm", &resolver_params)
            .assert_error()
            .to_string()
            .contains("do not match the original run")
    );
    assert_recovery_unavailable(&fixture, &resolver_successor);

    let trusted_values = openengine_cluster_protocol::StaticConnectionValues::new(BTreeMap::from(
        [(fixture.field.clone(), "fresh".to_owned())],
    ))
    .assert_value();
    let params = RunResumeParams {
        connections: BTreeMap::from([(fixture.key.clone(), trusted_values)]),
        ..params
    };
    fixture
        .connector
        .prepare_workspace_recovery_resume("vm", &params)
        .assert_value();
    assert_eq!(
        fixture
            .registry
            .recovery_authorization(&fixture.target.id, &successor_run_id)
            .assert_value(),
        fixture.trusted
    );
    assert_eq!(
        fixture
            .connector
            .authorize_workspace_recovery_requirements(
                "vm",
                &successor_run_id,
                fixture.trusted.clone(),
            )
            .assert_value(),
        fixture.trusted
    );
    fixture
        .connector
        .revoke_workspace_recovery("vm", &successor_run_id)
        .assert_value();
    assert_recovery_unavailable(&fixture, &successor_run_id);
}

fn assert_recovery_unavailable(fixture: &DirectRecoveryFixture, run_id: &RunId) {
    assert!(matches!(
        fixture
            .registry
            .recovery_authorization(&fixture.target.id, run_id),
        Err(TargetConnectorError::RecoveryAuthorizationUnavailable)
    ));
}

#[tokio::test]
async fn named_target_recovery_and_checkpoints_require_advertisement_before_dialing() {
    let registry = MemoryRegistry::default();
    registry.insert(target()).assert_value();
    let authority = FakeAuthority::new("wss://target.example/oecp");
    let dialer = FakeDialer::default();
    let backend = NamedTargetCliBackend::new(NativeV2TargetConnector::new(
        registry,
        authority.clone(),
        dialer.clone(),
    ));
    let source_run_id = run_request().run_id;

    for error in [
        backend
            .run_resume(
                Some("prod"),
                RunResumeParams {
                    run_id: source_run_id.clone(),
                    successor_run_id: RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c992"),
                    connections: BTreeMap::new(),
                    connection_resolver: None,
                    github_token: None,
                    from: None,
                },
            )
            .await
            .assert_error(),
        backend
            .run_discard_workspace(
                Some("prod"),
                RunDiscardWorkspaceParams {
                    run_id: source_run_id.clone(),
                },
            )
            .await
            .assert_error(),
    ] {
        assert!(
            error
                .to_string()
                .contains("target does not advertise workspace recovery")
        );
    }
    let error = backend
        .run_checkpoints(
            Some("prod"),
            RunCheckpointsParams {
                run_id: source_run_id,
                after: None,
                limit: None,
            },
        )
        .await
        .assert_error();
    assert!(
        error
            .to_string()
            .contains("target does not advertise workspace checkpoints")
    );
    assert!(authority.calls().is_empty());
    assert!(dialer.sessions.lock().assert_value().is_empty());
}

#[tokio::test]
async fn hosted_authority_uses_unified_discovery_and_run_scoped_oecp() {
    let root = temp_root();
    let (origin, server) = spawn_target_authority(15).await;
    let credentials = Arc::new(MemoryCredentialStore::default());
    let notifier = Arc::new(MemoryDeviceCodeNotifier::default());
    let authority = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        notifier.clone(),
        root.path("refresh-locks"),
    );
    let target = hosted_target("local", origin.clone());
    let request = exact_run_request();

    authority.login(&target).await.assert_value();
    assert_eq!(
        notifier.values(),
        vec![(format!("{origin}/activate"), "ABCD-EFGH".to_owned())]
    );
    let receipt = authority.submit(&target, &request).await.assert_value();
    assert_eq!(receipt.run_id, request.run_id);
    let session = authority
        .oecp_session(
            &target,
            &TargetOecpSessionRequest {
                run_id: Some(request.run_id.clone()),
            },
        )
        .await
        .assert_value();
    assert_eq!(
        session.endpoint(),
        format!(
            "ws://{}/native-v2/oecp",
            origin.trim_start_matches("http://")
        )
    );
    assert_eq!(
        credentials.get(&target.id).await.assert_value().as_deref(),
        Some("refresh-2")
    );
    assert_eq!(
        authority
            .workspace_recovery_session(
                &target,
                &TargetOecpSessionRequest {
                    run_id: Some(request.run_id.clone()),
                },
            )
            .await
            .assert_error()
            .to_string(),
        "target does not advertise workspace recovery"
    );

    assert_eq!(
        authority
            .workspace_checkpoints_session(
                &target,
                &TargetOecpSessionRequest {
                    run_id: Some(request.run_id.clone())
                },
            )
            .await
            .assert_error()
            .to_string(),
        "target does not advertise workspace checkpoints"
    );

    let requests = server.await.assert_value();
    assert_eq!(requests.len(), 15);
    assert!(
        requests
            .iter()
            .all(|request| !request.path.contains("capsule"))
    );
    assert_device_exchange(&requests);
    assert_submit_and_session_requests(&requests, &request.run_id);
}

#[tokio::test]
async fn direct_authority_skips_hosted_auth_and_all_authorization_headers() {
    let root = temp_root();
    let (origin, server) = spawn_direct_target_authority(9).await;
    let credentials = Arc::new(MemoryCredentialStore::default());
    let notifier = Arc::new(MemoryDeviceCodeNotifier::default());
    let authority = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        notifier.clone(),
        root.path("refresh-locks"),
    );
    let target = direct_target(origin);
    let request = exact_run_request();

    authority.discover(&target).await.assert_value();
    assert_eq!(
        authority.login(&target).await.assert_error().to_string(),
        "direct target does not use login"
    );
    assert_eq!(
        authority
            .submit(&target, &request)
            .await
            .assert_value()
            .run_id,
        request.run_id
    );
    authority
        .oecp_session(
            &target,
            &TargetOecpSessionRequest {
                run_id: Some(request.run_id.clone()),
            },
        )
        .await
        .assert_value();
    authority
        .workspace_recovery_session(
            &target,
            &TargetOecpSessionRequest {
                run_id: Some(request.run_id.clone()),
            },
        )
        .await
        .assert_value();
    authority
        .workspace_checkpoints_session(
            &target,
            &TargetOecpSessionRequest {
                run_id: Some(request.run_id),
            },
        )
        .await
        .assert_value();

    assert!(credentials.get(&target.id).await.assert_value().is_none());
    assert!(notifier.values().is_empty());
    let requests = server.await.assert_value();
    assert!(
        requests
            .iter()
            .all(|request| request.authorization.is_none())
    );
    assert!(
        requests
            .iter()
            .all(|request| request.path != "/.well-known/openengine-hosted-target")
    );
}

#[tokio::test]
async fn direct_authority_surfaces_bounded_run_rejection_feedback() {
    let root = temp_root();
    let (origin, server) = spawn_rejecting_direct_target_authority().await;
    let authority = test_http_authority(root.path("refresh-locks"));
    let error = authority
        .submit(&direct_target(origin), &exact_run_request())
        .await
        .assert_error();
    assert_eq!(
        error.to_string(),
        "target run request was rejected: required payload target issueNumber is not defined by a binding"
    );
    server.await.assert_value();
}

#[tokio::test]
async fn direct_target_rejects_hosted_controller_discovery() {
    let root = temp_root();
    let (origin, server) = spawn_target_authority(1).await;
    let authority = test_http_authority(root.path("refresh-locks"));
    let target = direct_target(origin);
    assert!(authority.discover(&target).await.is_err());
    server.await.assert_value();
}

fn assert_device_exchange(requests: &[CapturedHttpRequest]) {
    let request = requests
        .iter()
        .find(|request| request.path == "/oauth/token" && request.body.contains("device_code="))
        .assert_value();
    assert!(
        request
            .body
            .contains("device_token=22222222-2222-4222-8222-222222222222")
    );
    assert!(request.body.contains("device_label=zeroshot-cli"));
    assert!(request.body.contains("audience=controller"));
}

fn assert_submit_and_session_requests(requests: &[CapturedHttpRequest], run_id: &RunId) {
    let submit = requests
        .iter()
        .find(|request| request.path == "/native-v2/run")
        .assert_value();
    assert_eq!(submit.authorization.as_deref(), Some("Bearer access-2"));
    let request: serde_json::Value = serde_json::from_str(&submit.body).assert_value();
    assert_eq!(
        request.pointer("/submission/title").assert_value(),
        "Repair checkout"
    );
    assert_eq!(
        request.pointer("/submission/runtime/size").assert_value(),
        "medium"
    );
    assert_eq!(request.pointer("/runId").assert_value(), run_id.as_str());
    assert_eq!(
        request
            .pointer("/submission/source/revision")
            .assert_value(),
        "0123456789abcdef0123456789abcdef01234567"
    );
    assert!(
        request
            .pointer("/connections")
            .and_then(serde_json::Value::as_object)
            .is_some_and(serde_json::Map::is_empty)
    );
    let session = requests
        .iter()
        .find(|request| request.path == "/native-v2/oecp-session")
        .assert_value();
    assert_eq!(session.authorization.as_deref(), Some("Bearer access-2"));
    let session_request: serde_json::Value = serde_json::from_str(&session.body).assert_value();
    assert_eq!(
        session_request.pointer("/runId").assert_value(),
        run_id.as_str()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hosted_authorities_serialize_one_time_refresh_rotation_per_target() {
    let root = temp_root();
    let (origin, server) = spawn_target_authority(10).await;
    let credentials = Arc::new(RotatingCredentialStore::new("refresh-0"));
    let notifier = Arc::new(MemoryDeviceCodeNotifier::default());
    let lock_directory = root.path("refresh-locks");
    let first = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        notifier.clone(),
        lock_directory.clone(),
    );
    let second = TargetHttpControlAuthority::with_dependencies(
        credentials.clone(),
        notifier,
        lock_directory,
    );
    let target = hosted_target("local", origin);
    let request = TargetOecpSessionRequest::default();
    let (first_session, second_session) = tokio::join!(
        first.oecp_session(&target, &request),
        second.oecp_session(&target, &request)
    );
    first_session.assert_value();
    second_session.assert_value();
    assert_eq!(credentials.value(), "refresh-2");
    let requests = server.await.assert_value();
    let refresh_bodies = requests
        .iter()
        .filter(|request| {
            request.path == "/oauth/token" && request.body.contains("grant_type=refresh_token")
        })
        .map(|request| request.body.as_str())
        .collect::<Vec<_>>();
    assert_eq!(refresh_bodies.len(), 2);
    assert!(
        refresh_bodies
            .assert_at(0)
            .contains("refresh_token=refresh-0")
    );
    assert!(
        refresh_bodies
            .assert_at(1)
            .contains("refresh_token=refresh-1")
    );
    let authorizations = requests
        .iter()
        .filter(|request| request.path == "/native-v2/oecp-session")
        .filter_map(|request| request.authorization.as_deref())
        .collect::<Vec<_>>();
    assert_eq!(authorizations, ["Bearer access-1", "Bearer access-2"]);
}

#[tokio::test]
#[allow(clippy::result_large_err)]
async fn websocket_dialer_sends_bearer_only_to_same_target_authority() {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = listener.local_addr().assert_value();
    let (sent, received) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.assert_value();
        let websocket = accept_hdr_async(
            stream,
            move |request: &ServerRequest, response: ServerResponse| {
                let authorization = request
                    .headers()
                    .get(AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let _ = sent.send(authorization);
                Ok(response)
            },
        )
        .await
        .assert_value();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        drop(websocket);
    });
    let target = hosted_target("local", format!("http://{address}"));
    let session = TargetOecpAccess::new(
        format!("ws://{address}/oecp"),
        Some("secret".to_owned()),
        &target.access,
    )
    .assert_value();
    let transport = TargetOecpWebSocketDialer
        .dial(&target, session)
        .await
        .assert_value();
    assert_eq!(
        received.await.assert_value().as_deref(),
        Some("Bearer secret")
    );
    drop(transport);
    server.await.assert_value();
}

#[tokio::test]
#[allow(clippy::result_large_err)]
async fn direct_websocket_dialer_sends_no_authorization() {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = listener.local_addr().assert_value();
    let (sent, received) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.assert_value();
        let websocket = accept_hdr_async(
            stream,
            move |request: &ServerRequest, response: ServerResponse| {
                let authorization = request.headers().get(AUTHORIZATION).cloned();
                let _ = sent.send(authorization);
                Ok(response)
            },
        )
        .await
        .assert_value();
        drop(websocket);
    });
    let target = direct_target(format!("http://{address}"));
    let session =
        TargetOecpAccess::new(format!("ws://{address}/oecp"), None, &target.access).assert_value();
    let transport = TargetOecpWebSocketDialer
        .dial(&target, session)
        .await
        .assert_value();
    assert!(received.await.assert_value().is_none());
    drop(transport);
    server.await.assert_value();
}

#[tokio::test]
async fn websocket_dialer_rejects_cross_authority_before_network() {
    let target = target();
    let session = TargetOecpAccess::new(
        "wss://other.example/oecp",
        Some("secret".to_owned()),
        &target.access,
    )
    .assert_value();
    let error = TargetOecpWebSocketDialer
        .dial(&target, session)
        .await
        .assert_error_with("cross-authority session unexpectedly dialed");
    assert!(matches!(error, TargetConnectorError::InvalidOecpEndpoint));
}
