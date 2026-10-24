use openengine_cluster_protocol::{FieldPath, GraphNode};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};

fn review() -> AdmittedRun {
    let mut admitted = crate::native_v2_runner::test_support::admitted();
    admitted.graph.root = BuiltinGraphTemplate::IssueReview
        .materialize(TemplateDelivery::Comment)
        .assert_value()
        .root;
    admitted.source = Some(
        serde_json::from_value(json!({
            "repository":"acme/project", "branch":"main", "revision":"a".repeat(40)
        }))
        .assert_value(),
    );
    admitted.initial_input = json!({
        "url":"https://github.com/acme/project/pull/17",
        "context":{"title":"Review","body":"Changes","reviewScope":{
            "baseRepository":"acme/project", "baseBranch":"main", "baseRevision":"b".repeat(40),
            "headRepository":"contributor/project", "headBranch":"feature", "headRevision":"a".repeat(40)
        }}
    });
    admitted
}

#[test]
fn unrelated_graph_input_never_authorizes_review_checkout_preparation() {
    let mut admitted = crate::native_v2_runner::test_support::admitted();
    admitted.initial_input = json!({"context":{"reviewScope":{"baseRevision":"caller data"}}});
    admitted.source = None;
    assert!(checkout_references(&admitted).assert_value().is_empty());
}

#[test]
fn review_checkout_uses_the_publishers_bound_caller_paths_and_exact_source() {
    let mut admitted = review();
    let GraphNode::Seq(root) = &mut admitted.graph.root else {
        panic!("review root")
    };
    let GraphNode::Choice(route) = &mut root.children.as_mut_slice()[1] else {
        panic!("review route")
    };
    let Some(GraphNode::Seq(continuation)) = route.otherwise.as_deref_mut() else {
        panic!("review continuation")
    };
    let GraphNode::Verifier(publisher) = &mut continuation.children.as_mut_slice()[0] else {
        panic!("publisher")
    };
    for binding in &mut publisher.input_bindings {
        let path = match binding.target.segments()[0].as_str() {
            "url" => "destination",
            "reviewScope" => "comparison",
            _ => continue,
        };
        binding.value = openengine_cluster_protocol::DataSelector::State {
            path: FieldPath::new(vec![
                openengine_cluster_protocol::FieldName::new(path).assert_value(),
            ])
            .assert_value(),
        };
    }
    admitted.initial_input = json!({
        "destination":admitted.initial_input["url"],
        "comparison":admitted.initial_input["context"]["reviewScope"]
    });
    assert_eq!(
        checkout_references(&admitted).assert_value(),
        ["refs/pull/17/head".to_owned(), "b".repeat(40)]
    );
    let source = admitted.source.take().assert_value();
    assert!(checkout_references(&admitted).assert_value().is_empty());
    admitted.source = Some(source);
    admitted.source.as_mut().unwrap().revision =
        SourceRevisionId::new("c".repeat(40)).assert_value();
    assert!(matches!(
        checkout_references(&admitted),
        Err(ReviewResolveError::Identity)
    ));
}

#[test]
fn repository_casing_does_not_relax_the_exact_branch_or_revision() {
    let mut admitted = review();
    admitted.initial_input["url"] = json!("https://github.com/ACME/Project/pull/17");
    admitted.initial_input["context"]["reviewScope"]["baseRepository"] = json!("Acme/PROJECT");
    assert_eq!(checkout_references(&admitted).assert_value().len(), 2);
    admitted.initial_input["context"]["reviewScope"]["baseBranch"] = json!("Main");
    assert!(checkout_references(&admitted).is_err());
    admitted.initial_input["context"]["reviewScope"]["baseBranch"] = json!("main");
    admitted.initial_input["context"]["reviewScope"]["headRevision"] = json!("c".repeat(40));
    assert!(checkout_references(&admitted).is_err());
}

#[cfg(unix)]
mod local_auth {
    use std::collections::BTreeMap;
    use openengine_cluster_protocol::{NodeName, RunId, RunSubmission};
    use crate::native_v2_admission::DeliveryPolicy;
    use crate::native_v2_candidate::test_support::{
        TestGitRepository, commit_all, git, git_output, path_text,
    };
    use crate::native_v2_contract::NodeRuntimeBinding;
    use crate::native_v2_supervisor::RunEnvironment;
    use super::*;

    const TOKEN: &str = "private-review-fixture";

    fn bootstrap(
        admitted: &AdmittedRun,
        repository: &TestGitRepository,
        declared: bool,
    ) -> PortableControllerBootstrap {
        let submission = RunSubmission {
            title: admitted.title.clone(),
            graph: BuiltinGraphTemplate::IssueReview
                .materialize(TemplateDelivery::Comment)
                .assert_value(),
            initial_input: admitted.initial_input.clone(),
            runtime: admitted.runtime.clone(),
            environment: None,
            source: admitted.source.clone(),
            submission_key: openengine_cluster_protocol::IdempotencyKey::new("review-auth")
                .assert_value(),
        };
        let mut values = if declared {
            serde_json::from_value(json!({"github":{"GH_TOKEN":TOKEN}})).assert_value()
        } else {
            BTreeMap::new()
        };
        if let Some(key) = admitted
            .source
            .as_ref()
            .and_then(|source| source.remote.as_ref())
            .and_then(|remote| remote.connection.as_ref())
        {
            values.insert(
                key.clone(),
                GitTransportCredentials::Https {
                    username: "x-access-token".to_owned(),
                    password: TOKEN.to_owned(),
                }
                .connection_values()
                .assert_value(),
            );
        }
        let environment = RunEnvironment::exact_for_admitted(admitted, values).assert_value();
        PortableControllerBootstrap {
            checkpoint: None,
            run_id: RunId::new("review-auth"),
            delivery_run_id: RunId::new("review-auth"),
            adopt_existing_delivery: false,
            submission,
            environment,
            native_environment: BTreeMap::new(),
            github_token: None,
            github_token_file: None,
            delivery_validator: None,
            git_credentials: None,
            workspace: repository.workspace.clone(),
            workspace_lease: repository.root.child("workspace.lock"),
            checkpoint_repository: repository.root.child("checkpoints"),
            memory_storage: None,
            storage: repository.root.child("storage"),
            delivery_policy: DeliveryPolicy::Optional,
        }
    }

    fn authenticated_git(
        bootstrap: &mut PortableControllerBootstrap,
        repository: &TestGitRepository,
    ) {
        let path = std::env::var("PATH").assert_value();
        let executable = crate::execution::platform::find_executable(
            "git",
            &BTreeMap::from([("PATH".to_owned(), path)]),
            &repository.workspace,
        )
        .assert_value();
        let bin = repository.root.child("bin");
        std::fs::create_dir(&bin).assert_value();
        let script = r#"#!/bin/sh
while test "$1" = -c || test "$1" = -C; do
    shift 2
done
if test "$1" = fetch; then
    if test "$REVIEW_FIXTURE_SOURCE_ONLY" = 1; then
        test -z "$GH_TOKEN" || exit 71
    else
        test "$GH_TOKEN" = "private-review-fixture" || exit 71
    fi
    test "$GIT_CONFIG_COUNT" = 4 || exit 72
    test "$GIT_CONFIG_KEY_0" = credential.helper && test -z "$GIT_CONFIG_VALUE_0" || exit 73
    test "$GIT_CONFIG_VALUE_2" = "AUTHORIZATION: basic EXPECTED_BASIC" || exit 74
    test -z "$GIT_ASKPASS" && test -z "$SSH_ASKPASS" || exit 75
    test "$4" = "https://github.com/acme/project.git" || exit 76
    printf 'authenticated-fetch\n' >> "$REVIEW_FIXTURE_MARKER"
    exec "$REVIEW_FIXTURE_GIT" fetch --no-tags -- "$REVIEW_FIXTURE_REMOTE" "$5" "$6"
fi
exec "$REVIEW_FIXTURE_GIT" "$@"
"#
        .replace(
            "EXPECTED_BASIC",
            &crate::native_v2_delivery::git_auth::encode_basic_credential(TOKEN),
        );
        openengine_cluster_testkit::fixture::write_executable(&bin.join("git"), script, 0o700)
            .assert_value();
        bootstrap.native_environment = BTreeMap::from([
            ("PATH".to_owned(), path_text(&bin).to_owned()),
            (
                "HOME".to_owned(),
                path_text(repository.root.path()).to_owned(),
            ),
            (
                "GIT_ASKPASS".to_owned(),
                "/unavailable-native-helper".to_owned(),
            ),
            (
                "SSH_ASKPASS".to_owned(),
                "/unavailable-native-helper".to_owned(),
            ),
            (
                "REVIEW_FIXTURE_GIT".to_owned(),
                path_text(&executable).to_owned(),
            ),
            (
                "REVIEW_FIXTURE_REMOTE".to_owned(),
                path_text(&repository.remote).to_owned(),
            ),
            (
                "REVIEW_FIXTURE_MARKER".to_owned(),
                path_text(&repository.root.child("authenticated")).to_owned(),
            ),
        ]);
    }

    fn review_source(repository: &TestGitRepository, base: &str, authority: &str) -> AdmittedRun {
        let mut admitted = review();
        admitted.source.as_mut().unwrap().revision =
            SourceRevisionId::new(repository.base.clone()).assert_value();
        admitted.initial_input["context"]["reviewScope"]["headRevision"] = json!(repository.base);
        admitted.initial_input["context"]["reviewScope"]["baseRevision"] = json!(base);
        admitted.runtime.nodes_mut().clear();
        admitted.runtime.nodes_mut().insert(
            NodeName::new("deliver").assert_value(),
            serde_json::from_value::<NodeRuntimeBinding>(
                if matches!(authority, "declared" | "source") {
                    json!({"kind":"comment_delivery","connections":{"github":["GH_TOKEN"]}})
                } else {
                    json!({"kind":"comment_delivery"})
                },
            )
            .assert_value(),
        );
        if authority == "source" {
            admitted.source.as_mut().unwrap().remote = Some(
                serde_json::from_value(json!({
                    "url":"https://github.com/acme/project.git", "connection":"review-source"
                }))
                .assert_value(),
            );
        }
        admitted
    }

    fn assert_workspace_unchanged(repository: &TestGitRepository) {
        assert_eq!(
            git_output(&repository.workspace, &["rev-parse", "HEAD"]),
            repository.base
        );
        assert!(git_output(&repository.workspace, &["status", "--porcelain"]).is_empty());
    }

    #[tokio::test]
    async fn scope_free_issue_comments_need_neither_git_nor_credentials() {
        for absent in [true, false] {
            let repository = TestGitRepository::candidate();
            let mut admitted = review_source(&repository, &repository.base, "trusted");
            admitted.initial_input["url"] = json!("https://github.com/acme/project/issues/17");
            admitted.initial_input["context"]["reviewScope"] = json!({});
            if absent {
                let mut publisher = crate::native_v2_admission::comment_node(&admitted.graph.root)
                    .assert_value()
                    .clone();
                publisher
                    .input_bindings
                    .retain(|binding| binding.target.segments()[0].as_str() != "reviewScope");
                admitted.graph.root = GraphNode::Verifier(publisher);
            }
            let mut bootstrap = bootstrap(&admitted, &repository, false);
            bootstrap.github_token = Some(format!("{TOKEN}\n"));
            assert!(checkout_references(&admitted).assert_value().is_empty());
            prepare_local_checkout(&admitted, &bootstrap)
                .await
                .assert_value();
            assert!(!bootstrap.storage.exists());
            assert_workspace_unchanged(&repository);
        }
    }

    #[tokio::test]
    async fn missing_git_refuses_before_reading_credentials_or_mutating_the_checkout() {
        let repository = TestGitRepository::candidate();
        let admitted = review_source(&repository, &repository.base, "trusted");
        let mut bootstrap = bootstrap(&admitted, &repository, false);
        bootstrap.github_token = Some(format!("{TOKEN}\n"));
        let Err(PortableControllerError::Io(error)) =
            prepare_local_checkout(&admitted, &bootstrap).await
        else {
            panic!("missing Git must refuse preparation")
        };
        assert_eq!(error.to_string(), "review checkout requires Git");
        assert!(!bootstrap.storage.exists());
        assert_workspace_unchanged(&repository);
    }

    #[tokio::test]
    async fn failed_authenticated_fetch_keeps_head_and_hides_transport_diagnostics() {
        let repository = TestGitRepository::candidate();
        let admitted = review_source(&repository, &repository.base, "declared");
        let mut bootstrap = bootstrap(&admitted, &repository, true);
        authenticated_git(&mut bootstrap, &repository);
        bootstrap.native_environment.insert(
            "REVIEW_FIXTURE_REMOTE".to_owned(),
            path_text(&repository.root.child(TOKEN)).to_owned(),
        );
        let Err(PortableControllerError::Io(error)) =
            prepare_local_checkout(&admitted, &bootstrap).await
        else {
            panic!("missing remote must refuse preparation")
        };
        assert_eq!(
            error.to_string(),
            "could not fetch captured PR head/base for review"
        );
        assert_eq!(
            std::fs::read_to_string(repository.root.child("authenticated")).assert_value(),
            "authenticated-fetch\n"
        );
        assert_workspace_unchanged(&repository);
    }

    #[tokio::test]
    async fn invalid_or_unavailable_authority_refuses_before_git_without_disclosing_it() {
        for token_file in [false, true] {
            let repository = TestGitRepository::candidate();
            let admitted = review_source(&repository, &repository.base, "trusted");
            let mut bootstrap = bootstrap(&admitted, &repository, false);
            authenticated_git(&mut bootstrap, &repository);
            if token_file {
                bootstrap.github_token_file = Some(
                    crate::native_v2_delivery::TrustedGitHubTokenFile::new(
                        repository.root.child(TOKEN),
                        &repository.workspace,
                    )
                    .assert_value(),
                );
            } else {
                bootstrap.github_token = Some(format!("{TOKEN}\n"));
            }
            let Err(PortableControllerError::Io(error)) =
                prepare_local_checkout(&admitted, &bootstrap).await
            else {
                panic!("invalid authority must refuse preparation")
            };
            assert_eq!(
                error.to_string(),
                "review checkout credentials are unavailable or invalid"
            );
            assert!(!repository.root.child("authenticated").exists());
            assert!(!bootstrap.storage.exists());
            assert_workspace_unchanged(&repository);
        }
    }

    #[tokio::test]
    async fn private_missing_base_fetch_uses_declared_trusted_and_token_file_authority() {
        for authority in ["declared", "trusted", "file", "source"] {
            let repository = TestGitRepository::candidate();
            git(
                &repository.remote,
                &["update-ref", "refs/pull/17/head", &repository.base],
            );
            let seed = repository.root.child("seed");
            std::fs::write(seed.join("upstream"), "new comparison base").assert_value();
            commit_all(&seed, "advance comparison base");
            git(&seed, &["push", "origin", "main"]);
            let base = git_output(&seed, &["rev-parse", "HEAD"]);
            assert!(
                !std::process::Command::new("git")
                    .arg("-C")
                    .arg(&repository.workspace)
                    .args(["cat-file", "-e", &format!("{base}^{{commit}}")])
                    .output()
                    .assert_value()
                    .status
                    .success()
            );
            let declared = matches!(authority, "declared" | "source");
            let admitted = review_source(&repository, &base, authority);
            let mut bootstrap = bootstrap(&admitted, &repository, declared);
            authenticated_git(&mut bootstrap, &repository);
            if authority == "source" {
                bootstrap
                    .native_environment
                    .insert("REVIEW_FIXTURE_SOURCE_ONLY".to_owned(), "1".to_owned());
                bootstrap.git_credentials = Some(GitTransportCredentials::Https {
                    username: "trusted-must-not-win".to_owned(),
                    password: "trusted-must-not-win".to_owned(),
                });
            }
            if authority != "declared" {
                bootstrap.github_token = Some(TOKEN.to_owned());
            }
            let token_path = repository.root.child("token");
            if authority == "file" {
                bootstrap.github_token = None;
                use std::io::Write;
                crate::execution::platform::private_file(
                    &token_path,
                    crate::execution::platform::FileAccess::CreateNew,
                )
                .assert_value()
                .write_all(TOKEN.as_bytes())
                .assert_value();
            }
            bootstrap.github_token_file = Some(
                crate::native_v2_delivery::TrustedGitHubTokenFile::new(
                    token_path,
                    &repository.workspace,
                )
                .assert_value(),
            );
            prepare_local_checkout(&admitted, &bootstrap)
                .await
                .assert_value();
            assert_eq!(
                git_output(
                    &repository.workspace,
                    &["rev-parse", "refs/zeroshot/review/base"]
                ),
                base
            );
            assert_eq!(
                git_output(&repository.workspace, &["rev-parse", "HEAD"]),
                repository.base
            );
            assert_eq!(
                std::fs::read_to_string(repository.root.child("authenticated")).assert_value(),
                "authenticated-fetch\n"
            );
        }
    }
}
