use openengine_cluster_protocol::{RunConnectionRequirements, RunResumeRuntime};

use super::*;

fn replacement(uniform: bool, harness: &str) -> Value {
    let agent = json!({
        "model":"corrected-model", "effort":"high",
        "connections":{"replacement":["OPENAI_API_KEY"]}
    });
    if uniform {
        let mut runtime = agent;
        runtime["harness"] = json!(harness);
        runtime["provider"] = json!("openai");
        runtime
    } else {
        let mut agent = agent;
        agent["kind"] = json!("agent");
        json!({"harness":harness, "provider":"openai", "nodes":{"worker":agent}})
    }
}

fn resume_command(files: &FixtureFiles, uniform: bool, target: Option<&str>) -> NativeV2CliCommand {
    let flag = if uniform {
        "--uniform-runtime-config"
    } else {
        "--runtime-config"
    };
    let mut argv = args(&["resume", "run-failed", flag]);
    argv.push(files.runtime.as_os_str().to_owned());
    if let Some(target) = target {
        argv.extend(args(&["--target", target]));
    }
    parse_native_v2_args(argv).assert_value()
}

#[tokio::test]
async fn resume_runtime_dispatches_both_forms_with_only_fresh_replacement_credentials() {
    for uniform in [false, true] {
        for target in [None, Some("direct")] {
            let files = FixtureFiles::new(graph(), json!({"task":"continue"}));
            let runtime = replacement(uniform, "codex");
            std::fs::write(&files.runtime, serde_json::to_vec(&runtime).assert_value())
                .assert_value();
            let expected: RunResumeRuntime = serde_json::from_value(json!({
                "kind":if uniform {"uniform"} else {"exact"}, "runtime":runtime
            }))
            .assert_value();
            let original: RunConnectionRequirements = serde_json::from_value(json!({
                "original":["OLD_API_KEY"]
            }))
            .assert_value();
            let backend = FakeBackend::with_resume_requirements(original);
            let environment = |name: &str| match name {
                "OPENAI_API_KEY" => Some(OsString::from("fresh-replacement-value")),
                "GH_TOKEN" if target.is_none() => None,
                _ => panic!("unexpected credential lookup: {name}"),
            };
            assert_eq!(
                execute_with_environment(
                    resume_command(&files, uniform, target),
                    &backend,
                    &environment
                )
                .await
                .assert_value(),
                CliOutcome::Completed
            );
            let calls = backend.calls();
            let [
                Call::Status { .. },
                Call::Resume {
                    target: actual_target,
                    params,
                    ..
                },
                Call::Logs { .. },
            ] = calls.as_slice()
            else {
                panic!("expected one successor and its memory-log check");
            };
            assert_eq!(actual_target.as_deref(), target);
            assert_eq!(params.run_id.as_str(), "run-failed");
            assert_ne!(params.run_id, params.successor_run_id);
            assert_eq!(params.runtime.as_ref(), Some(&expected));
            assert_eq!(
                serde_json::to_value(&params.connections).assert_value(),
                json!({
                    "replacement":{"OPENAI_API_KEY":"fresh-replacement-value"}
                })
            );
            assert!(params.github_token.is_none());
        }
    }
}

#[tokio::test]
async fn resume_runtime_rejects_file_and_provider_errors_before_backend_or_credentials() {
    for uniform in [false, true] {
        for target in [None, Some("direct")] {
            let files = FixtureFiles::new(graph(), json!({"task":"continue"}));
            for (contents, code) in [
                (b"{".to_vec(), "json.invalid"),
                (
                    serde_json::to_vec(&replacement(uniform, "unregistered-harness"))
                        .assert_value(),
                    "request.invalid",
                ),
            ] {
                std::fs::write(&files.runtime, contents).assert_value();
                let backend = FakeBackend::default();
                let error = execute_with_environment(
                    resume_command(&files, uniform, target),
                    &backend,
                    &|name| panic!("preflight read credential {name}"),
                )
                .await
                .assert_error();
                let diagnostic = serde_json::to_value(error.diagnostic()).assert_value();
                assert_eq!(diagnostic["kind"], "invalid_request", "{error}");
                assert_eq!(diagnostic["code"], code, "{error}");
                assert!(backend.calls().is_empty());
            }
        }
    }
}

#[test]
fn resume_runtime_requirement_union_preserves_source_grants_and_deduplicates_fields() {
    let base: RunConnectionRequirements = serde_json::from_value(json!({
        "source":["GITLAB_TOKEN"], "replacement":["OPENAI_API_KEY","SHARED"]
    }))
    .assert_value();
    let original = base.clone();
    for uniform in [false, true] {
        let mut replacement = replacement(uniform, "codex");
        let binding = if uniform {
            &mut replacement
        } else {
            &mut replacement["nodes"]["worker"]
        };
        binding["connections"]["replacement"] = json!(["OPENAI_API_KEY", "NEW_API_KEY"]);
        let runtime: RunResumeRuntime = serde_json::from_value(json!({
            "kind":if uniform {"uniform"} else {"exact"}, "runtime":replacement
        }))
        .assert_value();
        let merged = merge_resume_requirements(&base, &runtime);
        assert_eq!(
            serde_json::to_value(merged).assert_value(),
            json!({
                "source":["GITLAB_TOKEN"],
                "replacement":["NEW_API_KEY", "OPENAI_API_KEY", "SHARED"]
            })
        );
        assert_eq!(base, original);
    }
}
