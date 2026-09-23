use openengine_cluster_protocol::{RUN_HISTORY_KIND, TargetRunHistoryDiscovery, TargetRunHistoryRoutes};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

fn extensions(
    base_url: &str,
    kind: &str,
    routes: TargetRunHistoryRoutes,
) -> TargetDiscoveryExtensions {
    TargetDiscoveryExtensions {
        run_history: Some(TargetRunHistoryDiscovery {
            kind: kind.to_owned(),
            base_url: base_url.to_owned(),
            route_templates: routes,
        }),
        ..TargetDiscoveryExtensions::default()
    }
}

fn routes() -> TargetRunHistoryRoutes {
    TargetRunHistoryRoutes {
        list: "/native-v2/history{?after}".into(),
        detail: "/native-v2/history/{run_id}".into(),
        page: "/native-v2/history/{run_id}/page{?after}".into(),
    }
}

#[test]
fn expands_only_the_advertised_same_origin_routes() {
    let origin = Url::parse("https://target.example").assert_value();
    let descriptor = build_run_history_descriptor(
        &origin,
        &extensions("https://target.example/api/", RUN_HISTORY_KIND, routes()),
    )
    .assert_value();
    let id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
    assert_eq!(
        descriptor.list_url(None).assert_value().as_str(),
        "https://target.example/api/native-v2/history"
    );
    assert_eq!(
        descriptor.list_url(Some(&id)).assert_value().as_str(),
        "https://target.example/api/native-v2/history?after=018f5e78-7f95-7c22-8d98-3f15af20c991"
    );
    assert_eq!(
        descriptor.detail_url(&id).assert_value().as_str(),
        "https://target.example/api/native-v2/history/018f5e78-7f95-7c22-8d98-3f15af20c991"
    );
    assert_eq!(
        descriptor
            .page_url(&id, &Cursor::new("v2:17"))
            .assert_value()
            .as_str(),
        "https://target.example/api/native-v2/history/018f5e78-7f95-7c22-8d98-3f15af20c991/page?after=v2%3A17"
    );
}

#[test]
fn rejects_absent_wrong_kind_cross_origin_and_unsafe_routes() {
    let origin = Url::parse("https://target.example").assert_value();
    assert!(build_run_history_descriptor(&origin, &TargetDiscoveryExtensions::default()).is_err());
    for (base_url, kind) in [
        ("https://target.example", "zeroshot.run-history/v2"),
        ("https://attacker.example", RUN_HISTORY_KIND),
    ] {
        assert!(
            build_run_history_descriptor(&origin, &extensions(base_url, kind, routes())).is_err()
        );
    }

    for invalid in [
        TargetRunHistoryRoutes {
            page: "/../history/{run_id}{?after}".into(),
            ..routes()
        },
        TargetRunHistoryRoutes {
            detail: "/native-v2/history/detail".into(),
            ..routes()
        },
    ] {
        assert!(
            build_run_history_descriptor(
                &origin,
                &extensions("https://target.example", RUN_HISTORY_KIND, invalid),
            )
            .is_err()
        );
    }
}

#[test]
fn route_templates_fail_closed_on_shape_and_query_mismatches() {
    let cases = [
        ("/runs", false, true),
        ("/runs/{run_id}{?after}", false, true),
        ("/runs?after={after}", false, true),
        ("/runs/{run_id}{?after}", true, false),
        ("/runs", true, false),
        ("/runs/{run_id}/{run_id}", true, false),
        ("/runs/{run_id}/page", true, true),
        ("/runs/page{?after}", true, true),
        ("/runs/{other}{?after}", true, true),
    ];
    for (template, requires_run_id, allows_after) in cases {
        assert!(
            compile_route(template, requires_run_id, allows_after).is_err(),
            "accepted incompatible template {template}"
        );
    }
}

#[test]
fn compiled_routes_reject_missing_inputs_and_non_hierarchical_bases() {
    let origin = Url::parse("https://target.example").assert_value();
    let detail = compile_route("/runs/{run_id}", true, false).assert_value();
    assert_eq!(
        detail
            .expand(&origin, None, None)
            .assert_error()
            .to_string(),
        "run-history route template is incompatible"
    );
    let list = compile_route("/runs", false, false).assert_value();
    assert!(list.expand(&origin, None, Some("v2:1")).is_err());
    assert!(
        list.expand(
            &Url::parse("mailto:history@target.example").assert_value(),
            None,
            None,
        )
        .is_err()
    );
}
