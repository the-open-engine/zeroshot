use super::*;
use crate::native_v2_contract::{ExecutionId, ExecutionRef, NodeInstanceId, NodeInvocation};
use crate::native_v2_delivery::{DeliveryMode, contract};
use crate::native_v2_delivery::git::test_support::{ConfigLocation, GitFixture};
use crate::native_v2_runner::{NodeRunRequest, NodeRunner, ResolvedEnvironment};
use openengine_cluster_protocol::{NodeName, WorkerOutcome, WorkerRef};

#[tokio::test]
async fn local_delivery_uses_caller_snapshot_from_minimal_controller_environment() {
    const CHILD: &str = "ZEROSHOT_TEST_LOCAL_GIT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let mut environment = BTreeMap::new();
        crate::execution::platform::process_environment(&mut environment);
        environment.insert("PATH".to_owned(), std::env::var("PATH").assert_value());
        if let Ok(directory) = std::env::var("TMPDIR") {
            environment.insert("TMPDIR".to_owned(), directory);
        }
        let module = module_path!().split_once("::").assert_value().1;
        let output = std::process::Command::new(std::env::current_exe().assert_value())
            .env_clear()
            .envs(environment)
            .env(CHILD, "1")
            .args(["--exact", &format!("{module}::local_delivery_uses_caller_snapshot_from_minimal_controller_environment"), "--nocapture"])
            .output()
            .assert_value();
        assert!(
            output.status.success(),
            "minimal controller test failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    }
    assert!(std::env::var_os("HOME").is_none());
    assert!(std::env::var_os("GIT_CONFIG_GLOBAL").is_none());
    // PR/merge delivery must normalize the candidate before integrating an advanced target.
    for (owner_scoped, mode, autocrlf) in [
        (false, DeliveryMode::Push, "true"),
        (true, DeliveryMode::Push, "true"),
        #[cfg(unix)]
        (false, DeliveryMode::PullRequest, "true"),
        #[cfg(unix)]
        (false, DeliveryMode::Merge, "input"),
    ] {
        let fixture = GitFixture::new(
            &format!("[core]\n autocrlf = {autocrlf}\n[merge]\n log = true\n"),
            ConfigLocation::Home,
        );
        #[cfg(unix)]
        let mut fixture = fixture;
        let base = fixture.seed();
        let remote = fixture.root.path().join("remote.git");
        fixture.git(&[
            "clone",
            "--bare",
            fixture.workspace.to_str().assert_value(),
            remote.to_str().assert_value(),
        ]);
        let remote_url = url::Url::from_directory_path(&remote)
            .assert_value()
            .to_string();
        let review = mode != DeliveryMode::Push;
        let alias = if review {
            "https://github.com/fixture/project.git"
        } else {
            "https://git.example/project.git"
        };
        fixture.git(&[
            "config",
            "--global",
            &format!("url.{remote_url}.insteadOf"),
            alias,
        ]);
        #[cfg(unix)]
        if review {
            advance_github_target(&mut fixture, &remote);
        }
        fixture.edit();
        // Every possible object ID collides with at least one captured credential. These
        // values stay private while Git parsers still receive the original IDs and paths.
        let mut native_environment = fixture.environment.clone();
        native_environment.insert("GIT_USERNAME".to_owned(), ".git".to_owned());
        for digit in "0123456789abcdef".chars() {
            native_environment.insert(format!("GIT_COLLISION_{digit}_TOKEN"), digit.to_string());
        }
        let mut remote_input = json!({"url": alias});
        if review {
            remote_input["forge"] =
                json!({"provider": "source.github", "repository": "fixture/project"});
        }
        let node = json!({
            "kind": "verifier", "name": "deliver", "worker": mode.worker_ref(),
            "input": {"kind": "null"}, "output": contract::delivery_result_schema(mode).assert_value(),
            "inputBindings": [], "writeBindings": [], "timeoutMs": 30000, "attempts": 1,
            "signals": {"delivery": contract::delivery_signal_labels(mode).assert_value()},
            "diagnostic": contract::delivery_diagnostic_schema().assert_value(),
        });
        let submission = serde_json::from_value(json!({
            "title": "Local Git configuration", "initialInput": null,
            "graph": full_graph(vec![node, success_node()]),
            "runtime": {"harness": "codex", "provider": "openai", "size": "small", "nodes": {
                "deliver": {"kind": "git_delivery"}
            }},
            "source": {"repository": "fixture/project", "branch": "main", "revision": base,
                "remote": remote_input},
            "submissionKey": "local-git-context"
        }))
        .assert_value();
        let admitted = NativeV2Admission.admit(submission).await.assert_value();
        let run_id = RunId::new("local-git-context");
        let runner = build_local_candidate_config(
            LocalProcessCandidateRequest {
                admitted: &admitted,
                delivery_run_id: run_id.clone(),
                adopt_existing_delivery: false,
                workspace: &fixture.workspace,
                storage: &fixture.root.path().join("storage"),
                memory_storage: None,
                github_token: review.then(|| "fixture-github-token".to_owned()),
                github_token_file: None,
                delivery_validator: None,
                git_credentials: None,
                native_environment: &native_environment,
            },
            owner_scoped,
            None,
        )
        .assert_value();
        let name = NodeName::new("deliver").assert_value();
        let binding = admitted.runtime.nodes().get(&name).assert_value().clone();
        let mut handle = runner
            .start(NodeRunRequest {
                environment: ResolvedEnvironment::exact(&binding, BTreeMap::new()).assert_value(),
                invocation: NodeInvocation {
                    reference: ExecutionRef {
                        run_id,
                        node: name,
                        node_instance: NodeInstanceId::new(1).assert_value(),
                        execution: ExecutionId::new(1).assert_value(),
                    },
                    worker: WorkerRef::new(mode.worker_ref()).assert_value(),
                    instructions: None,
                    input: Value::Null,
                    binding,
                },
            })
            .await
            .assert_value();
        let completion = handle.completion().await.assert_value();
        let expected = if review { "repair_required" } else { "pushed" };
        assert!(
            matches!(&completion.outcome, WorkerOutcome::Verifier { output, .. } if output["outcome"] == expected),
            "{:?}",
            completion.outcome
        );
        let revision = String::from_utf8(fixture.git(&["rev-parse", "HEAD"]))
            .assert_value()
            .trim()
            .to_owned();
        fixture
            .assert_committed(&fixture.system_git(), &revision, b"first\nchanged\n")
            .await;
        assert_eq!(
            fixture.git(&["diff", "--numstat", &base, &revision, "--", "sample.txt"]),
            b"1\t1\tsample.txt\n"
        );
        assert!(!fixture.workspace.join(".gitattributes").exists());
        let local_config =
            String::from_utf8(fixture.git(&["config", "--local", "--list"])).assert_value();
        assert!(!local_config.contains("core.autocrlf="));
        if review {
            // The authority commits HEAD^1 before the adapter's ordinary prepare_revision path.
            assert_eq!(
                fixture.git(&["show", "HEAD^1:sample.txt"]),
                b"first\nchanged\n"
            );
            assert_eq!(
                fixture.git(&["diff", "--numstat", &base, "HEAD^1", "--", "sample.txt"]),
                b"1\t1\tsample.txt\n"
            );
            assert_eq!(fixture.git(&["show", "HEAD:upstream.txt"]), b"upstream\n");
            // merge.log lives only in the captured global config, proving raw merge commands inherit it.
            let message =
                String::from_utf8(fixture.git(&["show", "-s", "--format=%B"])).assert_value();
            assert!(
                message.contains("test: advance captured target"),
                "{message}"
            );
        }
        runner.close_session().await;
    }
}

#[cfg(unix)]
fn advance_github_target(fixture: &mut GitFixture, remote: &Path) {
    fixture.git(&["checkout", "-b", "upstream"]);
    std::fs::write(fixture.workspace.join("upstream.txt"), b"upstream\n").assert_value();
    fixture.git(&["add", "--all"]);
    fixture.git(&["commit", "-m", "test: advance captured target"]);
    fixture.git(&["push", remote.to_str().assert_value(), "HEAD:main"]);
    let revision = String::from_utf8(fixture.git(&["rev-parse", "HEAD"])).assert_value();
    fixture.git(&["checkout", "main"]);
    let directory = fixture.root.path().join("bin");
    std::fs::create_dir(&directory).assert_value();
    let reference =
        json!({"ref": "refs/heads/main", "object": {"sha": revision.trim(), "type": "commit"}});
    openengine_cluster_testkit::fixture::write_executable(
        &directory.join("gh"),
        format!(
            r#"#!/bin/sh
test -z "${{GIT_CONFIG_NOSYSTEM-}}${{XDG_CONFIG_HOME-}}" || exit 91
case "$2" in
  repos/fixture/project/pulls) printf '[]\n' ;;
  repos/fixture/project/git/ref/heads/main) printf '%s\n' '{reference}' ;;
  *) printf 'gh: Not Found (HTTP 404)\n' >&2; exit 1 ;;
esac
"#
        ),
        0o700,
    )
    .assert_value();
    let path = std::env::join_paths(
        std::iter::once(directory).chain(std::env::split_paths(&fixture.environment["PATH"])),
    )
    .assert_value();
    fixture
        .environment
        .insert("PATH".to_owned(), path.into_string().assert_value());
}
