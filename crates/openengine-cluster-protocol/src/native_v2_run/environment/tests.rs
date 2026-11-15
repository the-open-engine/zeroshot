use super::*;
use crate::RuntimePlan;
use serde_json::json;

#[test]
fn environment_rejects_reserved_names_oversized_scripts_and_ambiguous_credentials() {
    for name in ["PATH", "HOME", "ZEROSHOT_TOOLS", "CODEX_HOME", "TMPDIR"] {
        let environment: RuntimeEnvironment =
            serde_json::from_value(json!({"variables": {name: "override"}})).unwrap();
        assert!(environment.validate().is_err());
    }
    let oversized = RuntimeEnvironment {
        setup: Some("x".repeat(MAX_RUNTIME_SCRIPT_BYTES + 1)),
        ..Default::default()
    };
    assert!(oversized.validate().is_err());
    let environment: RuntimeEnvironment = serde_json::from_value(json!({
        "variables": {"REGISTRY_TOKEN":"public"}, "connections": {"registry":["REGISTRY_TOKEN"]}
    }))
    .unwrap();
    assert!(environment.validate().is_err());
    let environment: RuntimeEnvironment = serde_json::from_value(json!({
        "startup":"echo ready", "variables":{"NODE_ENV":"test"}, "connections":{"registry":["REGISTRY_TOKEN"]}
    })).unwrap();
    assert!(environment.validate().is_ok());
}

#[test]
fn preparation_connections_join_run_requirements_without_changing_profiles() {
    let mut value = json!({"harness":"codex", "provider":"openai", "size":"small", "nodes":{}});
    let runtime: RuntimePlan = serde_json::from_value(value.clone()).unwrap();
    let definition: RuntimeEnvironment =
        serde_json::from_value(json!({"connections":{"registry":["REGISTRY_TOKEN"]}})).unwrap();
    assert_eq!(
        crate::run_connection_requirements(&runtime, Some(&definition)).len(),
        1
    );
    assert!(runtime.connection_requirements().is_empty());
    for unsupported in [
        json!({"id":"saved"}),
        serde_json::to_value(definition).unwrap(),
    ] {
        value["environment"] = unsupported;
        assert!(serde_json::from_value::<RuntimePlan>(value.clone()).is_err());
    }
}

#[test]
fn preparation_failures_cannot_be_authored_as_graph_outcomes() {
    for reason in [
        "environment_setup_failed",
        "environment_startup_failed",
        "environment_preparation_timeout",
        "context_checkout_unavailable",
    ] {
        assert!(crate::FailReason::new(crate::EnumLabel::new(reason).unwrap()).is_err());
    }
}

#[test]
fn context_repositories_validate_locators_limits_branches_and_reserved_name() {
    for locator in [
        "acme/library",
        "https://github.com/acme/library.git",
        "git@github.com:acme/library.git",
        "git@[::1]:acme/library.git",
        "ssh://git@git.example/group/subgroup/library.git",
    ] {
        let environment: RuntimeEnvironment = serde_json::from_value(
            json!({"contextRepositories":[{"repository":locator,"branch":"@"}]}),
        )
        .unwrap();
        environment.validate().unwrap();
        assert!(
            environment.context_repositories[0]
                .repository_path()
                .unwrap()
                .ends_with("library")
        );
    }
    for value in [
        json!({"contextRepositories":[{"repository":"https://user:secret@github.com/acme/library"}]}),
        json!({"contextRepositories":[{"repository":"../library"}]}),
        json!({"contextRepositories":[{"repository":"file:///tmp/library"}]}),
        json!({"contextRepositories":[{"repository":"https://github.com"}]}),
        json!({"contextRepositories":[{"repository":"acme/library"},{"repository":"git@github.com:acme/library.git"}]}),
        json!({"contextRepositories":(0..17).map(|i| json!({"repository":format!("acme/library-{i}")})).collect::<Vec<_>>()}),
        json!({"contextRepositories":[{"repository":"acme/library","branch":".hidden"}]}),
        json!({"contextRepositories":[{"repository":"acme/library","branch":"main//branch"}]}),
        json!({"contextRepositories":[{"repository":"acme/library","branch":"HEAD"}]}),
        json!({"variables":{"ZEROSHOT_CONTEXT":"/override"}}),
    ] {
        let environment: RuntimeEnvironment = serde_json::from_value(value.clone()).unwrap();
        assert!(environment.validate().is_err(), "{value}");
    }
    for branch in ["-main", "bad..branch", "main branch"] {
        assert!(
            serde_json::from_value::<RuntimeEnvironment>(
                json!({"contextRepositories":[{"repository":"acme/library","branch":branch}]})
            )
            .is_err()
        );
    }
    assert_eq!(
        serde_json::to_value(RuntimeEnvironment::default()).unwrap(),
        json!({})
    );
}

#[test]
fn context_repository_percent_escapes_cannot_alias_duplicate_or_primary_sources() {
    let source = serde_json::from_value(json!({
        "repository":"octocat/Hello-World", "branch":"main", "revision":"a".repeat(40)
    }))
    .unwrap();
    for alias in [
        "https://github.com/octocat/%48ello-World.git",
        "https://github.com/%6fctocat/Hello-World.git",
        "https://github.com/octocat/unused/%2e%2e/Hello-World.git",
        "ssh://git@github.com/octocat/%48ello-World.git",
        "git@github.com:octocat/%48ello-World.git",
    ] {
        let environment: RuntimeEnvironment = serde_json::from_value(json!({
            "contextRepositories":[{"repository":"octocat/Hello-World"},{"repository":alias}]
        }))
        .unwrap();
        let error = environment.validate().unwrap_err();
        assert!(
            error.to_string().contains("percent escapes"),
            "{alias}: {error}"
        );
        assert!(
            environment.context_repositories[1]
                .validate_source(&source)
                .is_err()
        );
    }
}

#[test]
fn context_path_is_reserved_in_node_and_hook_connections() {
    for name in ["ZEROSHOT_CONTEXT", "zeroshot_context", "Zeroshot_Context"] {
        let error = serde_json::from_value::<crate::NodeRuntimeBinding>(json!({
            "kind":"agent", "model":"opaque-model", "connections":{"secrets":[name]}
        }))
        .unwrap_err();
        assert!(error.to_string().contains("ZEROSHOT_CONTEXT is reserved"));
        assert!(
            crate::DeclaredConnections::single(
                "secrets",
                crate::DeclaredEnvironment::new([
                    crate::EnvironmentVariableName::new(name).unwrap()
                ])
                .unwrap()
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<RuntimeEnvironment>(json!({"connections":{"secrets":[name]}}))
                .is_err()
        );
    }
}

#[test]
fn context_sources_match_order_repository_branch_and_authority() {
    use crate::{ResolvedSource, validate_context_sources};
    let source: ResolvedSource = serde_json::from_value(
        json!({"repository":"acme/app", "branch":"main", "revision":"a".repeat(40)}),
    )
    .unwrap();
    let environment: RuntimeEnvironment = serde_json::from_value(json!({"contextRepositories":[{"repository":"acme/library","branch":"stable"},{"repository":"acme/contracts"}]})).unwrap();
    let pin = |repository: &str, branch: &str| -> ResolvedSource {
        serde_json::from_value(
            json!({"repository":repository,"branch":branch,"revision":"b".repeat(40)}),
        )
        .unwrap()
    };
    let pins = vec![pin("acme/library", "stable"), pin("acme/contracts", "main")];
    validate_context_sources(Some(&environment), &pins, Some(&source)).unwrap();
    ContextRepository {
        repository: "ssh://git@github.com:22/acme/library.git".to_owned(),
        branch: None,
    }
    .validate_source(&source)
    .unwrap();
    for invalid in [
        vec![],
        pins[..1].to_vec(),
        vec![pins[1].clone(), pins[0].clone()],
        vec![pins[0].clone(), pins[1].clone(), pins[1].clone()],
        vec![pin("acme/library", "other"), pins[1].clone()],
    ] {
        assert!(validate_context_sources(Some(&environment), &invalid, Some(&source)).is_err());
    }
    assert!(validate_context_sources(None, &pins, Some(&source)).is_err());
    assert!(validate_context_sources(Some(&environment), &pins, None).is_err());
    for locator in [
        "acme/app",
        "ssh://git@github.com/acme/app.git",
        "ssh://git@github.com:22/acme/app.git",
        "https://other.example/acme/library.git",
    ] {
        let repository = ContextRepository {
            repository: locator.to_owned(),
            branch: None,
        };
        assert!(repository.validate_source(&source).is_err());
    }
}

#[test]
fn documented_context_environment_is_valid() {
    let guide = include_str!("../../../../../docs/guides/runtime-environments.md");
    let example = guide
        .split("```json\n")
        .nth(1)
        .unwrap()
        .split("```")
        .next()
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(example).unwrap();
    let environment: RuntimeEnvironment =
        serde_json::from_value(value["environment"].clone()).unwrap();
    environment.validate().unwrap();
    assert_eq!(environment.context_repositories.len(), 2);
}
