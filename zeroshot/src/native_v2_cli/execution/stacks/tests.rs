use std::path::PathBuf;

use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::*;

pub(super) fn test_manifest_json() -> Value {
    json!({
        "schema": STACK_SCHEMA,
        "title": "Release",
        "delivery": "merge",
        "source": {"repository": "owner/repo", "branch": "main"},
        "profile": "org:software-change",
        "expiresAt": "2026-09-11T00:00:00Z",
        "runs": {"build": {"input": {}}}
    })
}

fn test_run(input: i64, needs: &[&str]) -> StackManifestRun {
    StackManifestRun {
        input: json!({ "value": input }),
        needs: needs.iter().map(|value| (*value).to_owned()).collect(),
    }
}

fn tomorrow() -> String {
    let expires = OffsetDateTime::now_utc() + TimeDuration::days(1);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        expires.year(),
        u8::from(expires.month()),
        expires.day(),
        expires.hour(),
        expires.minute(),
        expires.second()
    )
}

fn write_manifest(directory: &Path, manifest: &Value) -> PathBuf {
    let path = directory.join("stack.json");
    std::fs::write(&path, serde_json::to_vec(manifest).assert_value()).assert_value();
    path
}

fn current_manifest(delivery: &str, runs: Value) -> Value {
    let mut manifest = test_manifest_json();
    manifest["expiresAt"] = json!(tomorrow());
    manifest["delivery"] = json!(delivery);
    manifest["runs"] = runs;
    manifest
}

fn validation_error(manifest: &Value) -> String {
    let directory = tempfile::tempdir().assert_value();
    let path = write_manifest(directory.path(), manifest);
    let mut output = Vec::new();
    match validate_file(&path, &mut output) {
        Ok(_) => panic!("manifest {manifest} must be rejected"),
        Err(error) => error.to_string(),
    }
}

fn plain_profile() -> RunProfile {
    RunProfile {
        id: "profile".into(),
        name: RunProfileName::new("plain").assert_value(),
        scope: RunProfileScope::Org,
        graph: crate::native_v2_cli::BuiltinGraphTemplate::SingleWorker
            .materialize(crate::native_v2_cli::TemplateDelivery::None)
            .assert_value(),
        runtime: serde_json::from_value(
            json!({"harness":"codex", "provider":"openai", "size":"small", "nodes":{}}),
        )
        .assert_value(),
        is_default: false,
    }
}

struct Compiled {
    printed: Value,
    submitted: Vec<(String, Vec<String>)>,
}

/// Validates like `zeroshot stack validate` and prepares the request `stack submit` would send.
fn compile(delivery: &str, runs: Value) -> Compiled {
    let directory = tempfile::tempdir().assert_value();
    let path = write_manifest(directory.path(), &current_manifest(delivery, runs));
    let mut output = Vec::new();
    assert_eq!(
        validate_file(&path, &mut output).assert_value(),
        CliOutcome::Completed
    );
    let request = prepared_request(
        IdempotencyKey::new("stack").assert_value(),
        load_manifest(&path).assert_value(),
        &plain_profile(),
        &|_| None,
    )
    .assert_value();
    Compiled {
        printed: serde_json::from_slice(&output).assert_value(),
        submitted: request
            .runs
            .iter()
            .map(|run| {
                (
                    run.name.as_str().to_owned(),
                    run.needs
                        .iter()
                        .map(|need| need.as_str().to_owned())
                        .collect(),
                )
            })
            .collect(),
    }
}

fn expected(runs: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
    runs.iter()
        .map(|(name, needs)| {
            (
                (*name).to_owned(),
                needs.iter().map(|need| (*need).to_owned()).collect(),
            )
        })
        .collect()
}

fn stacked(runs: Value, order: &[&str], needs: &[(&str, &[&str])]) {
    let compiled = compile("stacked_pull_requests", runs);
    assert_eq!(compiled.printed, json!({"valid": true, "order": order}));
    assert_eq!(compiled.submitted, expected(needs));
}

#[test]
fn stacked_delivery_keeps_a_chain_in_dependency_order() {
    stacked(
        json!({
            "build": {"input": {}},
            "test": {"needs": ["build"], "input": {}},
            "deploy": {"needs": ["test"], "input": {}}
        }),
        &["build", "test", "deploy"],
        &[("build", &[]), ("test", &["build"]), ("deploy", &["test"])],
    );
}

#[test]
fn stacked_delivery_orders_independent_runs_by_name() {
    stacked(
        json!({"zeta": {"input": {}}, "alpha": {"input": {}}, "mid": {"input": {}}}),
        &["alpha", "mid", "zeta"],
        &[("alpha", &[]), ("mid", &["alpha"]), ("zeta", &["mid"])],
    );
}

#[test]
fn stacked_delivery_turns_a_diamond_into_one_stack() {
    stacked(
        json!({
            "base": {"input": {}},
            "right": {"needs": ["base"], "input": {}},
            "left": {"needs": ["base"], "input": {}},
            "top": {"needs": ["right", "left"], "input": {}}
        }),
        &["base", "left", "right", "top"],
        &[
            ("base", &[]),
            ("left", &["base"]),
            ("right", &["left"]),
            ("top", &["right"]),
        ],
    );
}

#[test]
fn stacked_delivery_places_a_run_after_all_three_parents() {
    stacked(
        json!({
            "assemble": {"needs": ["ui", "api", "docs"], "input": {}},
            "ui": {"input": {}},
            "api": {"input": {}},
            "docs": {"input": {}}
        }),
        &["api", "docs", "ui", "assemble"],
        &[
            ("api", &[]),
            ("docs", &["api"]),
            ("ui", &["docs"]),
            ("assemble", &["ui"]),
        ],
    );
}

#[test]
fn merge_delivery_keeps_fan_in_and_prints_no_order() {
    let compiled = compile(
        "merge",
        json!({
            "base": {"input": {}},
            "right": {"needs": ["base"], "input": {}},
            "left": {"needs": ["base"], "input": {}},
            "top": {"needs": ["right", "left"], "input": {}}
        }),
    );
    assert_eq!(compiled.printed, json!({"valid": true}));
    assert_eq!(
        compiled.submitted,
        expected(&[
            ("base", &[]),
            ("left", &["base"]),
            ("right", &["base"]),
            ("top", &["left", "right"]),
        ])
    );
}

#[test]
fn manifests_require_the_stack_schema_and_a_known_delivery() {
    let runs = json!({"build": {"input": {}}});
    for delivery in ["merge", "stacked_pull_requests"] {
        compile(delivery, runs.clone());
    }

    for schema in ["zeroshot.merge-plan/v1", "zeroshot.stack/v2"] {
        let mut manifest = current_manifest("merge", runs.clone());
        manifest["schema"] = json!(schema);
        assert_eq!(
            validation_error(&manifest),
            "invalid stack: schema must be \"zeroshot.stack/v1\""
        );
    }

    let mut missing = current_manifest("merge", runs.clone());
    missing.as_object_mut().assert_value().remove("delivery");
    let mut rejected = vec![missing];
    for value in [
        json!("stacked_pull_request"),
        json!("Merge"),
        json!("pull_request"),
        json!(null),
        json!(1),
    ] {
        let mut misspelled = current_manifest("merge", runs.clone());
        misspelled["delivery"] = value;
        rejected.push(misspelled);
    }
    for manifest in &rejected {
        assert_eq!(
            validation_error(manifest),
            "invalid stack: delivery must be \"merge\" or \"stacked_pull_requests\""
        );
    }

    let mut misspelled_key = current_manifest("merge", runs);
    let delivery = misspelled_key
        .as_object_mut()
        .assert_value()
        .remove("delivery")
        .assert_value();
    misspelled_key["delivry"] = delivery;
    let error = validation_error(&misspelled_key);
    assert!(error.contains("unknown field `delivry`"), "{error}");
    assert!(error.contains("`delivery`"), "{error}");
}

#[test]
fn manifest_json_rejects_unknown_fields_at_every_level() {
    let mut top_level = test_manifest_json();
    top_level["extra"] = json!(true);
    assert!(serde_json::from_value::<StackManifest>(top_level).is_err());

    let mut node = test_manifest_json();
    node["runs"]["build"]["outputFrom"] = json!("other");
    assert!(serde_json::from_value::<StackManifest>(node).is_err());
}

#[test]
fn manifest_compiler_preserves_static_inputs_and_dependencies() {
    let mut raw = BTreeMap::new();
    raw.insert("build".to_owned(), test_run(1, &[]));
    raw.insert("integrate".to_owned(), test_run(2, &["build"]));
    let runs = compile_runs(&RunTitle::new("Release").assert_value(), raw).assert_value();

    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].name.as_str(), "build");
    assert!(runs[0].needs.is_empty());
    assert_eq!(runs[1].name.as_str(), "integrate");
    assert_eq!(runs[1].needs[0].as_str(), "build");
    assert_eq!(runs[1].initial_input, json!({ "value": 2 }));
}

#[test]
fn manifest_compiler_canonicalizes_dependency_order() {
    let name = StackRunName::new("integrate").assert_value();
    let names = ["backend", "frontend", "integrate"]
        .map(|value| StackRunName::new(value).assert_value())
        .into_iter()
        .collect();
    let needs = compile_needs(
        &name,
        vec!["frontend".to_owned(), "backend".to_owned()],
        &names,
    )
    .assert_value();

    assert_eq!(
        needs.iter().map(StackRunName::as_str).collect::<Vec<_>>(),
        ["backend", "frontend"]
    );
}

#[test]
fn manifest_compiler_rejects_invalid_dependency_graphs() {
    let title = RunTitle::new("Release").assert_value();
    let mut missing = BTreeMap::new();
    missing.insert("build".to_owned(), test_run(1, &["absent"]));
    assert!(compile_runs(&title, missing).is_err());

    let mut self_edge = BTreeMap::new();
    self_edge.insert("build".to_owned(), test_run(1, &["build"]));
    assert!(compile_runs(&title, self_edge).is_err());

    let mut cycle = BTreeMap::new();
    cycle.insert("a".to_owned(), test_run(1, &["b"]));
    cycle.insert("b".to_owned(), test_run(2, &["a"]));
    cycle.insert("c".to_owned(), test_run(3, &[]));
    let runs = compile_runs(&title, cycle).assert_value();
    let error = match dependency_order(&runs) {
        Ok(order) => panic!("cycle produced order {order:?}"),
        Err(error) => error.to_string(),
    };
    assert_eq!(error, "invalid stack: runs must form an acyclic graph");

    for delivery in ["merge", "stacked_pull_requests"] {
        let manifest = current_manifest(
            delivery,
            json!({"a": {"needs": ["b"], "input": {}}, "b": {"needs": ["a"], "input": {}}}),
        );
        assert_eq!(
            validation_error(&manifest),
            "invalid stack: runs must form an acyclic graph"
        );
    }
}

#[test]
fn manifest_requires_scoped_profile_and_rfc3339_deadline() {
    assert!(parse_profile("org:software-change").is_ok());
    assert!(parse_profile("software-change").is_err());
    assert!(parse_profile("local:software-change").is_err());
    let now = OffsetDateTime::parse("2026-09-10T00:00:00Z", &Rfc3339).assert_value();
    assert!(validate_deadline_at("2026-09-11T00:00:00Z", now).is_ok());
    assert!(validate_deadline_at("2026-09-09T00:00:00Z", now).is_err());
    assert!(validate_deadline_at("2026-09-18T00:00:00Z", now).is_err());
    assert!(validate_deadline_at("tomorrow", now).is_err());
}

#[test]
fn cli_contract_manifest_validation_and_terminal_outcomes_are_closed() {
    let directory = tempfile::tempdir().assert_value();
    let mut manifest = test_manifest_json();
    manifest["expiresAt"] = json!(tomorrow());
    let manifest_path = write_manifest(directory.path(), &manifest);
    let mut output = Vec::new();
    assert_eq!(
        validate_file(&manifest_path, &mut output).assert_value(),
        CliOutcome::Completed
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output).assert_value(),
        json!({"valid":true})
    );

    let title = RunTitle::new("x".repeat(256)).assert_value();
    let name = StackRunName::new("build").assert_value();
    assert!(validate_derived_title(&title, &name).is_err());
    let names = [StackRunName::new("build").assert_value()]
        .into_iter()
        .collect();
    assert!(
        compile_needs(
            &name,
            vec!["dependency".to_owned(), "dependency".to_owned()],
            &names,
        )
        .unwrap_err()
        .to_string()
        .contains("more than once")
    );

    assert!(compile_runs(&RunTitle::new("Release").assert_value(), BTreeMap::new()).is_err());
    let too_many = (0..=MAX_STACK_RUNS)
        .map(|index| (format!("run-{index}"), test_run(index as i64, &[])))
        .collect();
    assert!(compile_runs(&RunTitle::new("Release").assert_value(), too_many).is_err());

    for (state, expected) in [
        (StackState::Succeeded, CliOutcome::Finished),
        (StackState::Failed, CliOutcome::Failed),
        (StackState::Cancelled, CliOutcome::Failed),
        (StackState::Expired, CliOutcome::Failed),
        (StackState::Queued, CliOutcome::Completed),
        (StackState::Running, CliOutcome::Completed),
    ] {
        let stack: Stack = serde_json::from_value(json!({
            "stackId":"stack-contract",
            "title":"Release",
            "state":state,
            "repository":"owner/repo",
            "branch":"main",
            "submittedAt":"2026-09-10T00:00:00Z",
            "expiresAt":"2026-09-11T00:00:00Z",
            "runs":[]
        }))
        .assert_value();
        let outcome = outcome_for_stack(&stack);
        assert_eq!(outcome, expected);
        assert_eq!(
            foreground_outcome(outcome).is_err(),
            expected == CliOutcome::Failed
        );
    }
}
