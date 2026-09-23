use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

use super::*;

#[test]
fn recovery_routes_are_optional_and_escape_the_run_identity() {
    let origin = Url::parse("https://target.example").assert_value();
    let run_id = RunId::new("run/1");
    let missing = descriptor(
        &origin,
        json!({
            "kind": HOSTED_RUNS_KIND, "base_url": origin.as_str(), "route_templates": routes()
        }),
    )
    .assert_value();
    assert!(missing.resume_url(&run_id).is_err());
    assert!(missing.checkpoints_url(&run_id).is_err());
    assert!(missing.discard_workspace_url(&run_id).is_err());

    let present = descriptor_with_recovery(
        &origin,
        json!({
            "kind": HOSTED_RUNS_KIND, "base_url": origin.as_str(), "route_templates": routes()
        }),
        recovery_routes(),
    )
    .assert_value();
    assert_eq!(
        present.resume_url(&run_id).assert_value().as_str(),
        "https://target.example/runs/run%2F1/resume"
    );
    assert_eq!(
        present.checkpoints_url(&run_id).assert_value().as_str(),
        "https://target.example/runs/run%2F1/checkpoints"
    );
    assert_eq!(
        present
            .discard_workspace_url(&run_id)
            .assert_value()
            .as_str(),
        "https://target.example/runs/run%2F1/discard-workspace"
    );
}

#[test]
fn recovery_routes_reject_unsafe_paths_and_extra_template_variables() {
    let origin = Url::parse("https://target.example").assert_value();
    for name in ["resume", "checkpoints", "discard_workspace"] {
        for invalid in [
            "https://attacker.example/{run_id}",
            "/runs/{run_id}/../resume",
            "/runs/{run_id}/{checkpoint_id}",
        ] {
            assert!(
                descriptor_with_recovery(
                    &origin,
                    json!({
                        "kind": HOSTED_RUNS_KIND, "base_url": origin.as_str(),
                        "route_templates": routes()
                    }),
                    recovery_routes_with(name, invalid)
                )
                .is_err()
            );
        }
    }
}

#[test]
fn hosted_routes_expand_opaque_values_under_the_advertised_base_path() {
    let origin = Url::parse("https://target.example").assert_value();
    let descriptor = descriptor(
        &origin,
        json!({
            "kind": HOSTED_RUNS_KIND,
            "base_url": "https://target.example/api/",
            "route_templates": routes()
        }),
    )
    .assert_value();

    assert_eq!(
        descriptor.list_url().assert_value().as_str(),
        "https://target.example/api/native-v2/runs"
    );
    assert_eq!(
        descriptor
            .watch_url(&RunId::new("run/1"), Some(&Cursor::new("cloud:7")))
            .assert_value()
            .as_str(),
        "https://target.example/api/native-v2/runs/run%2F1/watch?from_cursor=cloud%3A7"
    );
    assert_eq!(
        descriptor
            .logs_url(
                &RunId::new("run/1"),
                Some(&Cursor::new("cloud:8")),
                Some(&ExecutionRef::new("worker/1").assert_value()),
            )
            .assert_value()
            .as_str(),
        "https://target.example/api/native-v2/runs/run%2F1/logs?from_cursor=cloud%3A8&execution=worker%2F1"
    );
}

#[test]
fn hosted_routes_reject_missing_cross_origin_or_unsafe_capabilities() {
    let origin = Url::parse("https://target.example").assert_value();
    let invalid = [
        Value::Null,
        json!({
            "kind": "zeroshot.hosted-runs/v2",
            "base_url": "https://target.example",
            "route_templates": routes()
        }),
        json!({
            "kind": HOSTED_RUNS_KIND,
            "base_url": "https://attacker.example",
            "route_templates": routes()
        }),
        json!({
            "kind": HOSTED_RUNS_KIND,
            "base_url": "https://target.example",
            "route_templates": routes_with("status", "/native-v2/runs")
        }),
        json!({
            "kind": HOSTED_RUNS_KIND,
            "base_url": "https://target.example",
            "route_templates": routes_with("watch", "/../runs/{run_id}/watch{?from_cursor}")
        }),
        json!({
            "kind": HOSTED_RUNS_KIND,
            "base_url": "https://target.example",
            "route_templates": routes_with("logs", "/runs/{run_id}/logs{?execution}")
        }),
    ];

    for hosted_runs in invalid {
        assert!(descriptor(&origin, hosted_runs).is_err());
    }
}

fn descriptor(
    origin: &Url,
    hosted_runs: Value,
) -> Result<HostedRunsDescriptor, TargetAuthorityError> {
    let extensions = serde_json::from_value::<TargetDiscoveryExtensions>(json!({
        "hosted_runs": hosted_runs
    }))
    .assert_value();
    build_hosted_runs_descriptor(origin, &extensions)
}

fn descriptor_with_recovery(
    origin: &Url,
    hosted_runs: Value,
    recovery: Value,
) -> Result<HostedRunsDescriptor, TargetAuthorityError> {
    let extensions = serde_json::from_value::<TargetDiscoveryExtensions>(json!({
        "hosted_runs": hosted_runs,
        "hosted_workspace_recovery": {
            "kind": HOSTED_WORKSPACE_RECOVERY_KIND,
            "route_templates": recovery
        }
    }))
    .assert_value();
    build_hosted_runs_descriptor(origin, &extensions)
}

fn routes() -> Value {
    json!({
        "list": "/native-v2/runs",
        "status": "/native-v2/runs/{run_id}",
        "watch": "/native-v2/runs/{run_id}/watch{?from_cursor}",
        "logs": "/native-v2/runs/{run_id}/logs{?from_cursor,execution}",
        "force": "/native-v2/runs/{run_id}/force"
    })
}

fn routes_with(name: &str, value: &str) -> Value {
    let mut routes = routes();
    routes
        .as_object_mut()
        .assert_value()
        .insert(name.to_owned(), Value::String(value.to_owned()));
    routes
}

fn recovery_routes_with(name: &str, value: &str) -> Value {
    let mut routes = recovery_routes();
    routes
        .as_object_mut()
        .assert_value()
        .insert(name.to_owned(), Value::String(value.to_owned()));
    routes
}

fn recovery_routes() -> Value {
    json!({
        "resume": "/runs/{run_id}/resume",
        "checkpoints": "/runs/{run_id}/checkpoints",
        "discard_workspace": "/runs/{run_id}/discard-workspace"
    })
}
