use super::*;
use openengine_cluster_protocol::{TargetRuntimeEnvironmentRoutes, TargetRuntimeEnvironmentsDiscovery};
use openengine_cluster_testkit::assertions::AssertValue;

fn discovery(base: &str, route: &str) -> TargetDiscoveryExtensions {
    TargetDiscoveryExtensions {
        runtime_environments: Some(TargetRuntimeEnvironmentsDiscovery {
            kind: RUNTIME_ENVIRONMENTS_KIND.into(),
            base_url: base.into(),
            route_templates: TargetRuntimeEnvironmentRoutes { show: route.into() },
        }),
        ..Default::default()
    }
}

#[test]
fn resource_discovery_keeps_scope_and_id_in_exact_same_origin_segments() {
    let origin = Url::parse("https://target.example").assert_value();
    let wire = discovery(
        "https://target.example",
        "/workspaces/{scope}/environments/{environment_id}",
    );
    let descriptor = build_environments_descriptor(&origin, &wire)
        .assert_value()
        .assert_value();
    let id = EnvironmentId::new("env/branch?query#fragment").assert_value();
    let url = descriptor.show(RunProfileScope::Org, &id).assert_value();
    assert_eq!(
        url.as_str(),
        "https://target.example/workspaces/org/environments/env%2Fbranch%3Fquery%23fragment"
    );
    for id in [".", ".."] {
        assert!(
            descriptor
                .show(
                    RunProfileScope::User,
                    &EnvironmentId::new(id).assert_value()
                )
                .is_err()
        );
    }
}

#[test]
fn resource_discovery_rejects_foreign_authority_and_ambiguous_templates() {
    let origin = Url::parse("https://target.example").assert_value();
    for route in [
        "//foreign/{scope}/{environment_id}",
        "/env/{environment_id}",
        "/env/{scope}/{scope}/{environment_id}",
        "/env/{scope}/{environment_id}?query=1",
        "/../{scope}/{environment_id}",
        "/env/{scope}/{unknown}",
    ] {
        assert!(
            build_environments_descriptor(&origin, &discovery("https://target.example", route))
                .is_err(),
            "{route}"
        );
    }
    assert!(
        build_environments_descriptor(
            &origin,
            &discovery("https://foreign.example", "/env/{scope}/{environment_id}")
        )
        .is_err()
    );
    assert!(
        build_environments_descriptor(&origin, &TargetDiscoveryExtensions::default())
            .assert_value()
            .is_none()
    );
}
