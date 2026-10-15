use openengine_cluster_protocol::{
    ConnectionKey, EnvironmentVariableName, GIT_TRANSPORT_KIND, GitRemoteUrl,
    GitTransportCredentials, ResolvedSource, RunResumeParams, RunStatusResult, RunSubmission,
    StaticConnectionValues, TargetAuthentication, TargetDiscoveryDocument,
    TargetDiscoveryExtensions, TargetRunRequest,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde::de::DeserializeOwned;
use schemars::JsonSchema;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn assert_wire_and_schema<T: DeserializeOwned + JsonSchema>(wire: Value, valid: bool) {
    let schema = serde_json::to_value(schemars::schema_for!(T)).assert_value();
    assert_eq!(
        jsonschema::validator_for(&schema)
            .assert_value()
            .is_valid(&wire),
        valid,
        "schema: {wire}"
    );
    assert_eq!(
        serde_json::from_value::<T>(wire.clone()).is_ok(),
        valid,
        "wire: {wire}"
    );
}

fn generic_source(url: &str) -> Value {
    json!({
        "repository": "nested/team/project:opaque-42", "branch": "main", "revision": "a".repeat(64),
        "remote": {"url": url, "connection": "source-access", "forge": {
            "provider": "source.contributed", "repository": "opaque/repository:42"
        }}
    })
}

fn submission() -> Value {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/native-v2-lane-submission.json"))
            .assert_value();
    fixture["submission"].clone()
}

#[test]
fn generic_sources_keep_opaque_ids_exact_revision_and_authored_remote() {
    let wire = generic_source("ssh://alice@git.example:2222/nested/team/project.git");
    assert_wire_and_schema::<ResolvedSource>(wire.clone(), true);
    let source: ResolvedSource = serde_json::from_value(wire.clone()).assert_value();
    assert!(source.requires_git_transport());
    assert_eq!(serde_json::to_value(&source).assert_value(), wire);
    assert_eq!(source.revision.as_str().len(), 64);
}

#[test]
fn absent_remote_retains_the_legacy_github_grammar_and_bytes() {
    let source = openengine_cluster_testkit::native_v2_source_fixture();
    let wire = serde_json::to_value(&source).assert_value();
    assert!(!source.requires_git_transport());
    assert!(wire.get("remote").is_none());
    assert_wire_and_schema::<ResolvedSource>(wire.clone(), true);
    let decoded: ResolvedSource = serde_json::from_value(wire.clone()).assert_value();
    assert_eq!(
        serde_json::to_vec(&source).assert_value(),
        serde_json::to_vec(&decoded).assert_value()
    );
    for repository in ["opaque-id", "team/group/project", "owner name/repository"] {
        let mut invalid = wire.clone();
        invalid["repository"] = json!(repository);
        assert_wire_and_schema::<ResolvedSource>(invalid, false);
    }
}

#[test]
fn source_omission_and_null_never_materialize_a_dummy_repository() {
    for source in [None, Some(Value::Null)] {
        let mut wire = submission();
        wire.as_object_mut().assert_value().remove("source");
        if let Some(source) = source {
            wire["source"] = source;
        }
        assert_wire_and_schema::<RunSubmission>(wire.clone(), true);
        let decoded: RunSubmission = serde_json::from_value(wire).assert_value();
        assert!(decoded.source.is_none());
        assert!(!decoded.requires_git_transport());
        assert!(
            serde_json::to_value(&decoded)
                .assert_value()
                .get("source")
                .is_none()
        );
    }
    let mut projection = serde_json::to_value(RunStatusResult {
        run_id: openengine_cluster_protocol::RunId::new("local-folder"),
        title: openengine_cluster_protocol::RunTitle::new("Local folder").assert_value(),
        source: None,
        size: openengine_cluster_protocol::RunSize::Small,
        at_cursor: openengine_cluster_protocol::Cursor::new("v2:0"),
        status: openengine_cluster_protocol::RunStatus::Admitted {},
        workspace_recovery: Default::default(),
    })
    .assert_value();
    assert!(projection.get("source").is_none());
    assert_wire_and_schema::<RunStatusResult>(projection.clone(), true);
    projection["source"] = Value::Null;
    assert_wire_and_schema::<RunStatusResult>(projection, true);
}

#[test]
fn https_ssh_and_scp_locators_preserve_authored_paths_and_usernames() {
    for remote in [
        "https://git.example/nested/team/project.git",
        "https://git.example:8443/team/project%20name.git",
        "ssh://alice@git.example:2222/nested/team/project.git",
        "git@git.example:team/project.git",
        "git@git.example:/srv/git/team/project.git",
        "git@[2001:db8::1]:team/project.git",
    ] {
        let url = GitRemoteUrl::new(remote).assert_value();
        assert_eq!(url.as_str(), remote);
        assert_wire_and_schema::<GitRemoteUrl>(json!(remote), true);
    }
}

#[test]
fn remote_locators_refuse_credentials_local_paths_and_executable_transports() {
    for remote in [
        "",
        "../repo",
        "/srv/repo",
        "C:\\repo",
        "C:/repo",
        "C:repo",
        "file:///srv/repo",
        "git://git.example/repo",
        "http://git.example/repo",
        "ext::command",
        "helper::repository",
        "ssh://git:secret@git.example/repo",
        "https:///git.example/repo",
        "ssh://@git.example/repo",
        "alice@@git.example:repo",
        "https://secret@git.example/repo",
        "https://@git.example/repo",
        "https://git.example/repo?token=secret",
        "ssh://git.example/repo#secret",
        "git.example:repo?secret",
        "git.example:",
        "-host:repo",
        "git@-host:repo",
        "ssh://git@-host/repo",
        "https://git.example/space path",
        "git.example:repo\n",
    ] {
        assert!(GitRemoteUrl::new(remote).is_err(), "accepted {remote:?}");
        assert_wire_and_schema::<GitRemoteUrl>(json!(remote), false);
    }
}

#[test]
fn transport_credentials_are_bounded_closed_and_redacted_in_every_envelope() {
    for wire in [
        json!({"kind":"https", "username":"private-user", "password":"https-secret"}),
        json!({"kind":"ssh", "privateKey":"ssh-private-secret", "knownHosts":"host-verification-secret"}),
    ] {
        assert_wire_and_schema::<GitTransportCredentials>(wire.clone(), true);
        let credentials: GitTransportCredentials =
            serde_json::from_value(wire.clone()).assert_value();
        assert_eq!(serde_json::to_value(&credentials).assert_value(), wire);
        let values = credentials.connection_values().assert_value();
        assert_eq!(
            GitTransportCredentials::from_connection_values(&values).assert_value(),
            Some(credentials.clone())
        );
        let request: TargetRunRequest = serde_json::from_value(json!({
            "runId":"run-1", "submission":submission(), "connections":{}, "gitCredentials":wire
        }))
        .assert_value();
        assert!(request.requires_git_transport());
        let resume: RunResumeParams = serde_json::from_value(json!({
            "runId":"run-1", "successorRunId":"run-2", "gitCredentials":wire
        }))
        .assert_value();
        for debug in [
            format!("{credentials:?}"),
            format!("{request:?}"),
            format!("{resume:?}"),
        ] {
            assert!(debug.contains("[REDACTED]"));
            for secret in [
                "private-user",
                "https-secret",
                "ssh-private-secret",
                "host-verification-secret",
            ] {
                assert!(!debug.contains(secret));
            }
        }
        assert!(
            serde_json::to_value(request.submission)
                .assert_value()
                .get("gitCredentials")
                .is_none()
        );
    }
}

#[test]
fn incomplete_mixed_or_oversized_credential_shapes_fail_closed() {
    for wire in [
        json!({"kind":"https", "username":"user"}),
        json!({"kind":"https", "username":"user", "password":""}),
        json!({"kind":"https", "username":"user", "password":"secret\u{0}value"}),
        json!({"kind":"https", "username":"user", "password":"x".repeat(65_537)}),
        json!({"kind":"https", "username":"user", "password":"secret", "privateKey":"key"}),
        json!({"kind":"ssh", "privateKey":"key", "knownHosts":""}),
        json!({"kind":"ssh", "privateKey":"key", "knownHosts":"hosts", "password":"secret"}),
        json!({"kind":"native_local"}),
    ] {
        assert_wire_and_schema::<GitTransportCredentials>(wire, false);
    }
}

#[test]
fn named_connection_conversion_rejects_partial_or_mixed_transport_authority() {
    for fields in [
        vec![("GIT_USERNAME", "user")],
        vec![("GIT_PASSWORD", "secret")],
        vec![
            ("GIT_USERNAME", "user"),
            ("GIT_PASSWORD", "secret"),
            ("GIT_SSH_PRIVATE_KEY", "key"),
        ],
    ] {
        let fields = fields
            .into_iter()
            .map(|(name, value)| {
                (
                    EnvironmentVariableName::new(name).assert_value(),
                    value.to_owned(),
                )
            })
            .collect();
        let values = StaticConnectionValues::new(fields).assert_value();
        assert!(GitTransportCredentials::from_connection_values(&values).is_err());
    }
    let unrelated = StaticConnectionValues::new(BTreeMap::from([(
        EnvironmentVariableName::new("API_KEY").assert_value(),
        "provider-secret".to_owned(),
    )]))
    .assert_value();
    assert!(
        GitTransportCredentials::from_connection_values(&unrelated)
            .assert_value()
            .is_none()
    );
}

#[test]
fn source_requirements_are_separate_from_agent_lane_declarations() {
    for (url, expected) in [
        (
            "https://git.example/nested/repo.git",
            ["GIT_PASSWORD", "GIT_USERNAME"],
        ),
        (
            "git@git.example:nested/repo.git",
            ["GIT_SSH_KNOWN_HOSTS", "GIT_SSH_PRIVATE_KEY"],
        ),
    ] {
        let mut wire = submission();
        wire["source"] = generic_source(url);
        let request: RunSubmission = serde_json::from_value(wire).assert_value();
        let key = ConnectionKey::new("source-access").assert_value();
        assert!(request.requires_git_transport());
        let requirements = request.connection_requirements();
        let fields: Vec<_> = requirements
            .get(&key)
            .assert_value()
            .iter()
            .map(|name| name.as_str())
            .collect();
        assert_eq!(fields, expected);
        assert!(!request.runtime.connection_requirements().contains_key(&key));
        assert_eq!(request.runtime.lanes().len(), 2);
    }
}

#[test]
fn git_transport_discovery_is_additive_strict_and_available_to_all_placements() {
    for authentication in [
        TargetAuthentication::None,
        TargetAuthentication::PrivateCapability,
        TargetAuthentication::HostedOauth,
    ] {
        let original = TargetDiscoveryDocument::direct(authentication);
        assert!(original.extensions.git_transport.is_none());
        let advertised = original.with_git_transport();
        assert!(!advertised.extensions.is_empty());
        let wire = serde_json::to_value(&advertised).assert_value();
        assert_eq!(
            wire["extensions"]["git_transport"],
            json!({"kind":GIT_TRANSPORT_KIND})
        );
        assert_eq!(
            serde_json::from_value::<TargetDiscoveryDocument>(wire).assert_value(),
            advertised
        );
    }
    let future: TargetDiscoveryExtensions = serde_json::from_value(json!({
        "git_transport":{"kind":"future/v2"}, "unrecognized_extension":{"kind":"future/v1","more":true}
    })).assert_value();
    assert_eq!(future.git_transport.assert_value().kind, "future/v2");
    assert!(
        serde_json::from_value::<TargetDiscoveryExtensions>(json!({
            "git_transport":{"kind":GIT_TRANSPORT_KIND,"protocols":["https"]}
        }))
        .is_err()
    );
}
