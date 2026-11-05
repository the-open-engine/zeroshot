use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::{Value, json};

use super::*;

#[test]
fn stack_routes_are_optional_and_expand_opaque_stack_ids() {
    let origin = Url::parse("https://target.example").assert_value();
    assert!(
        build_stacks_descriptor(&origin, &TargetDiscoveryExtensions::default())
            .assert_value()
            .is_none()
    );

    let descriptor = descriptor(&origin, STACKS_KIND, routes())
        .assert_value()
        .assert_value();
    let stack_id = StackId::new("stack/1");
    assert_eq!(
        descriptor.create_url().assert_value().as_str(),
        "https://target.example/api/stacks"
    );
    assert_eq!(
        descriptor.status_url(&stack_id).assert_value().as_str(),
        "https://target.example/api/stacks/stack%2F1"
    );
    assert_eq!(
        descriptor.force_url(&stack_id).assert_value().as_str(),
        "https://target.example/api/stacks/stack%2F1/force"
    );
}

#[test]
fn stack_routes_reject_incompatible_kinds_and_ambiguous_variables() {
    let origin = Url::parse("https://target.example").assert_value();
    for kind in ["zeroshot.stacks/v2", "zeroshot.merge-plans/v1"] {
        let wrong_kind = descriptor(&origin, kind, routes()).assert_error();
        assert_eq!(wrong_kind.to_string(), "stack discovery is incompatible");
    }

    for (route, value) in [
        ("create", "/stacks/{stack_id}"),
        ("status", "/stacks/status"),
        ("status", "/stacks/{stack_id}/{stack_id}"),
    ] {
        let error = descriptor(&origin, STACKS_KIND, routes_with(route, value)).assert_error();
        assert_eq!(
            error.to_string(),
            "stack route template declares unsupported variables"
        );
    }

    for (route, value) in [
        ("force", "/stacks/{stack_id}/{action}"),
        ("status", "/stacks/{plan_id}"),
    ] {
        let invalid_literal =
            descriptor(&origin, STACKS_KIND, routes_with(route, value)).assert_error();
        assert_eq!(
            invalid_literal.to_string(),
            "stack route template is invalid"
        );
    }
}

fn descriptor(
    origin: &Url,
    kind: &str,
    route_templates: Value,
) -> Result<Option<StacksDescriptor>, TargetAuthorityError> {
    let extensions = serde_json::from_value::<TargetDiscoveryExtensions>(json!({
        "stacks": {
            "kind": kind,
            "baseUrl": "https://target.example/api/",
            "routeTemplates": route_templates,
        }
    }))
    .assert_value();
    build_stacks_descriptor(origin, &extensions)
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
        "create": "/stacks",
        "status": "/stacks/{stack_id}",
        "force": "/stacks/{stack_id}/force",
    })
}
