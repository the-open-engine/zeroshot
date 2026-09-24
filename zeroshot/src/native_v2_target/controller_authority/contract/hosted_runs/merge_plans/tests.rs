use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};

use super::*;

#[test]
fn merge_plan_routes_are_optional_and_expand_opaque_plan_ids() {
    let origin = Url::parse("https://target.example").assert_value();
    assert!(
        build_merge_plans_descriptor(&origin, &TargetDiscoveryExtensions::default())
            .assert_value()
            .is_none()
    );

    let descriptor = descriptor(&origin, MERGE_PLANS_KIND, routes())
        .assert_value()
        .assert_value();
    let plan_id = MergePlanId::new("plan/1");
    assert_eq!(
        descriptor.create_url().assert_value().as_str(),
        "https://target.example/api/merge-plans"
    );
    assert_eq!(
        descriptor.status_url(&plan_id).assert_value().as_str(),
        "https://target.example/api/merge-plans/plan%2F1"
    );
    assert_eq!(
        descriptor.force_url(&plan_id).assert_value().as_str(),
        "https://target.example/api/merge-plans/plan%2F1/force"
    );
}

#[test]
fn merge_plan_routes_reject_incompatible_kinds_and_ambiguous_variables() {
    let origin = Url::parse("https://target.example").assert_value();
    let wrong_kind = descriptor(&origin, "zeroshot.merge-plans/v2", routes()).assert_error();
    assert_eq!(
        wrong_kind.to_string(),
        "merge-plan discovery is incompatible"
    );

    for (route, value) in [
        ("create", "/merge-plans/{plan_id}"),
        ("status", "/merge-plans/status"),
        ("status", "/merge-plans/{plan_id}/{plan_id}"),
    ] {
        let error = descriptor(&origin, MERGE_PLANS_KIND, routes_with(route, value)).assert_error();
        assert_eq!(
            error.to_string(),
            "merge-plan route template declares unsupported variables"
        );
    }

    let invalid_literal = descriptor(
        &origin,
        MERGE_PLANS_KIND,
        routes_with("force", "/merge-plans/{plan_id}/{action}"),
    )
    .assert_error();
    assert_eq!(
        invalid_literal.to_string(),
        "merge-plan route template is invalid"
    );
}

fn descriptor(
    origin: &Url,
    kind: &str,
    route_templates: Value,
) -> Result<Option<MergePlansDescriptor>, TargetAuthorityError> {
    let extensions = serde_json::from_value::<TargetDiscoveryExtensions>(json!({
        "merge_plans": {
            "kind": kind,
            "baseUrl": "https://target.example/api/",
            "routeTemplates": route_templates,
        }
    }))
    .assert_value();
    build_merge_plans_descriptor(origin, &extensions)
}

fn routes_with(name: &str, value: &str) -> Value {
    let Value::Object(mut routes) = routes() else {
        unreachable!("route fixture is always an object")
    };
    routes.insert(name.to_owned(), Value::String(value.to_owned()));
    Value::Object(routes)
}

fn routes() -> Value {
    json!({
        "create": "/merge-plans",
        "status": "/merge-plans/{plan_id}",
        "force": "/merge-plans/{plan_id}/force",
    })
}
