use std::sync::Arc;

use openengine_cluster_protocol::{RunProfileName, RunProfileScope};
use openengine_cluster_testkit::admission::graph_fixture;
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;

use super::super::credentials::test_support::{MemoryCredentialStore, MemoryDeviceCodeNotifier};
use super::*;

fn direct_target() -> TargetRecord {
    TargetRecord {
        id: "11111111-1111-4111-8111-111111111111".to_owned(),
        name: "vm".to_owned(),
        origin: "http://127.0.0.1:8080".to_owned(),
        access: TargetAccess::Direct,
    }
}

fn selector() -> RunProfileSelector {
    RunProfileSelector {
        scope: RunProfileScope::User,
        name: RunProfileName::new("contract").assert_value(),
    }
}

fn set_request() -> RunProfileSetRequest {
    RunProfileSetRequest {
        name: selector().name,
        scope: RunProfileScope::User,
        graph: graph_fixture("worker", json!({"kind":"null"})),
        runtime: serde_json::from_value(json!({
            "harness":"codex", "provider":"openai", "size":"small",
            "nodes":{"worker":{"kind":"agent","model":"opaque-model"}}
        }))
        .assert_value(),
        set_default: false,
    }
}

fn run_request() -> RunProfileRunRequest {
    serde_json::from_value(json!({
        "runId":"profile-contract-run",
        "profile":{"scope":"user","name":"contract"},
        "title":"Profile contract",
        "initialInput":null,
        "source":{
            "repository":"open-engine/zeroshot",
            "branch":"main",
            "revision":"0123456789abcdef0123456789abcdef01234567"
        },
        "submissionKey":"profile-contract",
        "connections":{}
    }))
    .assert_value()
}

#[test]
fn operations_select_their_exact_route_and_diagnostic_label() {
    let base = reqwest::Url::parse("https://target.example").assert_value();
    let routes = RunProfilesDescriptor {
        list: base.join("/profiles/list").assert_value(),
        show: base.join("/profiles/show").assert_value(),
        set: base.join("/profiles/set").assert_value(),
        delete: base.join("/profiles/delete").assert_value(),
        default: base.join("/profiles/default").assert_value(),
        run: base.join("/profiles/run").assert_value(),
    };
    for (operation, path, label) in [
        (ProfileOperation::List, "/profiles/list", "profile list"),
        (ProfileOperation::Show, "/profiles/show", "profile show"),
        (ProfileOperation::Set, "/profiles/set", "profile set"),
        (
            ProfileOperation::Delete,
            "/profiles/delete",
            "profile delete",
        ),
        (
            ProfileOperation::Default,
            "/profiles/default",
            "profile default",
        ),
        (ProfileOperation::Run, "/profiles/run", "profile run"),
    ] {
        assert_eq!(operation.route(&routes).path(), path);
        assert_eq!(operation.label(), label);
    }
}

#[tokio::test]
async fn direct_targets_reject_every_profile_wrapper_before_any_hosted_effect() {
    let root =
        openengine_cluster_testkit::TemporaryDirectory::for_test("target-direct-profile-refusal");
    let authority = TargetHttpControlAuthority::with_dependencies(
        Arc::new(MemoryCredentialStore::default()),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        root.path("locks"),
    );
    let target = direct_target();
    let expected = "direct target does not advertise profile management";

    assert_eq!(
        authority
            .profile_list(
                &target,
                RunProfileListRequest {
                    scope: RunProfileScope::User,
                },
            )
            .await
            .assert_error()
            .to_string(),
        expected
    );
    assert_eq!(
        authority
            .profile_show(&target, selector())
            .await
            .assert_error()
            .to_string(),
        expected
    );
    assert_eq!(
        authority
            .profile_set(&target, set_request())
            .await
            .assert_error()
            .to_string(),
        expected
    );
    assert_eq!(
        authority
            .profile_delete(&target, selector())
            .await
            .assert_error()
            .to_string(),
        expected
    );
    assert_eq!(
        authority
            .profile_default(
                &target,
                RunProfileDefaultRequest {
                    scope: RunProfileScope::User,
                    name: None,
                },
            )
            .await
            .assert_error()
            .to_string(),
        expected
    );
    assert_eq!(
        authority
            .profile_run(&target, &run_request())
            .await
            .assert_error()
            .to_string(),
        expected
    );
}
