use super::*;
use openengine_cluster_protocol::{ConnectionKey, SourceForgeIdentity};
use openengine_cluster_testkit::assertions::AssertError;

fn forge_source(provider: &str, base_url: &str, repository: &str) -> ResolvedSource {
    serde_json::from_value(json!({
        "repository":repository, "branch":"main", "revision":"a".repeat(40),
        "remote":{"url":format!("{base_url}/{repository}.git"), "forge":{
            "provider":provider, "repository":repository, "baseUrl":base_url}},
    }))
    .assert_value()
}

fn delivery_request(mode: DeliveryMode) -> RunSubmission {
    submission(
        graph(vec![delivery_verifier("deliver", mode), succeed("done")]),
        BTreeMap::from([(named("deliver"), delivery_binding())]),
    )
}

fn connections(key: &str, field: &str) -> DeclaredConnections {
    DeclaredConnections::single(
        key,
        DeclaredEnvironment::new([EnvironmentVariableName::new(field).assert_value()])
            .assert_value(),
    )
    .assert_value()
}

fn selected_forge(request: &RunSubmission) -> SourceForgeIdentity {
    request
        .source
        .as_ref()
        .and_then(|s| s.remote.as_ref())
        .and_then(|r| r.forge.as_ref())
        .assert_value()
        .clone()
}

#[test]
fn credentials_are_scoped_to_the_selected_forge_instance() {
    for (provider, base, repository, key, field) in [
        (
            "source.github",
            "https://ghe.example",
            "team/project",
            "github",
            "GH_TOKEN",
        ),
        (
            "source.gitlab",
            "https://gitlab.com",
            "group/sub/project",
            "gitlab",
            "GITLAB_TOKEN",
        ),
        (
            "source.gitlab",
            "https://gitlab.example/install",
            "group/sub/project",
            "gitlab",
            "GITLAB_TOKEN",
        ),
    ] {
        let mut request = delivery_request(DeliveryMode::PullRequest);
        request.source = Some(forge_source(provider, base, repository));
        materialize_delivery_access(&mut request).assert_value();
        assert_eq!(
            *request.runtime.nodes()[&named("deliver")].declared_connections(),
            connections(key, field)
        );
        assert_eq!(
            delivery_credential_destinations(&request).assert_value(),
            BTreeMap::from([(
                ConnectionKey::new(key).assert_value(),
                vec![selected_forge(&request)],
            )])
        );
    }
}

#[test]
fn authored_aliases_and_agent_credentials_retain_separate_authority() {
    let mut request = delivery_request(DeliveryMode::Merge);
    request.graph = graph(vec![
        null_step("worker", "worker.change@1"),
        delivery_verifier("deliver", DeliveryMode::Merge),
        succeed("done"),
    ]);
    request.source = Some(forge_source(
        "source.gitlab",
        "https://gitlab.example/install",
        "group/sub/project",
    ));
    let NodeRuntimeBinding::GitDelivery {
        connections: fields,
        ..
    } = request
        .runtime
        .nodes_mut()
        .get_mut(&named("deliver"))
        .assert_value()
    else {
        panic!("delivery fixture")
    };
    *fields = connections("tenant-forge", "GITLAB_TOKEN");
    request.runtime.nodes_mut().insert(
        named("worker"),
        binding_with_environment(["MODEL_TOKEN".into()]),
    );
    let original = request.runtime.clone();
    materialize_delivery_access(&mut request).assert_value();
    assert_eq!(request.runtime, original);
    assert_eq!(
        delivery_credential_destinations(&request).assert_value(),
        BTreeMap::from([(
            ConnectionKey::new("tenant-forge").assert_value(),
            vec![selected_forge(&request)],
        )])
    );
}

#[test]
fn missing_or_unselected_source_refuses_implicit_forge_credentials() {
    for mode in [DeliveryMode::PullRequest, DeliveryMode::Merge] {
        let mut missing = delivery_request(mode);
        missing.source = None;
        let original = missing.runtime.clone();
        assert_eq!(
            materialize_delivery_access(&mut missing),
            Err(NativeV2AdmissionError::DeliverySourceRequired)
        );
        assert_eq!(missing.runtime, original);
        assert_eq!(
            delivery_credential_destinations(&missing),
            Err(NativeV2AdmissionError::DeliverySourceRequired)
        );
        let mut unselected = delivery_request(mode);
        unselected.source = Some(
            serde_json::from_value(json!({
                "repository":"group/project", "branch":"main", "revision":"a".repeat(40),
                "remote":{"url":"https://gitlab.example/group/project.git"},
            }))
            .assert_value(),
        );
        let original = unselected.runtime.clone();
        for error in [
            materialize_delivery_access(&mut unselected).assert_error(),
            delivery_credential_destinations(&unselected).assert_error(),
        ] {
            assert!(
                matches!(error, NativeV2AdmissionError::InvalidSourceDelivery(detail) if detail.contains("selected source provider"))
            );
        }
        assert_eq!(unselected.runtime, original);
    }
}

#[test]
fn git_push_has_no_implicit_forge_api_requirement() {
    let mut request = delivery_request(DeliveryMode::Push);
    request.source = None;
    let original = request.runtime.clone();
    materialize_delivery_access(&mut request).assert_value();
    assert_eq!(request.runtime, original);
    assert!(
        delivery_credential_destinations(&request)
            .assert_value()
            .is_empty()
    );
}

#[test]
fn provider_without_default_credentials_refuses_without_mutating_runtime() {
    let mut request = delivery_request(DeliveryMode::PullRequest);
    request.source = Some(forge_source(
        "source.git",
        "https://git.example",
        "team/project",
    ));
    let original = request.runtime.clone();
    assert!(matches!(materialize_delivery_access(&mut request),
        Err(NativeV2AdmissionError::InvalidSourceDelivery(detail)) if detail.contains("no default delivery credential")));
    assert_eq!(request.runtime, original);
}

#[test]
fn comment_credentials_follow_destination_without_a_matching_git_source() {
    for source in [
        None,
        Some(forge_source(
            "source.github",
            "https://ghe.example",
            "team/project",
        )),
    ] {
        let mut request = super::comments::comment_submission();
        request.source = source;
        request.initial_input["url"] = json!("https://gitlab.example/group/sub/project/-/issues/7");
        materialize_delivery_access(&mut request).assert_value();
        assert_eq!(
            *request.runtime.nodes()[&named("deliver")].declared_connections(),
            connections("gitlab", "GITLAB_TOKEN")
        );
        let destinations = delivery_credential_destinations(&request).assert_value();
        let forge = &destinations[&ConnectionKey::new("gitlab").assert_value()][0];
        assert_eq!(forge.provider.as_str(), "source.gitlab");
        assert_eq!(forge.repository.as_str(), "group/sub/project");
        assert_eq!(
            forge.base_url.as_ref().assert_value().as_str(),
            "https://gitlab.example"
        );
    }
}

#[test]
fn comment_credentials_preserve_the_selected_installation_prefix() {
    let mut request = super::comments::comment_submission();
    request.source = Some(forge_source(
        "source.gitlab",
        "https://gitlab.example/install",
        "group/sub/project",
    ));
    request.initial_input["url"] =
        json!("https://gitlab.example/install/group/sub/project/-/issues/7");
    materialize_delivery_access(&mut request).assert_value();
    assert_eq!(
        delivery_credential_destinations(&request).assert_value(),
        BTreeMap::from([(
            ConnectionKey::new("gitlab").assert_value(),
            vec![selected_forge(&request)],
        )])
    );
}

#[tokio::test]
async fn mismatched_remote_identity_refuses_before_delivery_credentials() {
    for (remote_repository, base) in [
        ("group/other", "https://gitlab.example"),
        ("group/project", "https://another.example"),
    ] {
        let mut request = delivery_request(DeliveryMode::PullRequest);
        request.source = Some(serde_json::from_value(json!({
            "repository":"group/project", "branch":"main", "revision":"a".repeat(40),
            "remote":{"url":format!("https://gitlab.example/{remote_repository}.git"),
                "forge":{"provider":"source.gitlab", "repository":"group/project", "baseUrl":base}},
        })).assert_value());
        assert!(
            request.runtime.nodes()[&named("deliver")]
                .declared_connections()
                .is_empty()
        );
        assert!(matches!(
            NativeV2Admission.admit(request).await,
            Err(NativeV2AdmissionError::InvalidSourceDelivery(_))
        ));
    }
}
