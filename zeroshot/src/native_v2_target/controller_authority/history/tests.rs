use std::path::PathBuf;
use std::sync::Arc;

use openengine_cluster_protocol::{
    Cursor, RUN_HISTORY_KIND, RunId, TargetDiscoveryExtensions, TargetRunHistoryDiscovery,
    TargetRunHistoryRoutes,
};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::super::credentials::test_support::{MemoryCredentialStore, MemoryDeviceCodeNotifier};
use super::*;

fn descriptor() -> RunHistoryDescriptor {
    let origin = reqwest::Url::parse("https://target.example").assert_value();
    let extensions = TargetDiscoveryExtensions {
        run_history: Some(TargetRunHistoryDiscovery {
            kind: RUN_HISTORY_KIND.to_owned(),
            base_url: "https://target.example/api".to_owned(),
            route_templates: TargetRunHistoryRoutes {
                list: "/runs{?after}".to_owned(),
                detail: "/runs/{run_id}".to_owned(),
                page: "/runs/{run_id}/page{?after}".to_owned(),
            },
        }),
        ..TargetDiscoveryExtensions::default()
    };
    build_run_history_descriptor(&origin, &extensions).assert_value()
}

fn authority() -> TargetHttpControlAuthority {
    TargetHttpControlAuthority::with_dependencies(
        Arc::new(MemoryCredentialStore::default()),
        Arc::new(MemoryDeviceCodeNotifier::default()),
        PathBuf::from("unused-history-refresh-locks"),
    )
}

fn target(origin: &str) -> TargetRecord {
    TargetRecord {
        id: "11111111-1111-4111-8111-111111111111".to_owned(),
        name: "history".to_owned(),
        origin: origin.to_owned(),
        access: TargetAccess::Direct,
    }
}

#[test]
fn request_urls_preserve_the_exact_discovered_routes() {
    let descriptor = descriptor();
    let id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
    let cases = [
        (
            RunHistoryRequest::list(None),
            "https://target.example/api/runs",
        ),
        (
            RunHistoryRequest::list(Some(id.clone())),
            "https://target.example/api/runs?after=018f5e78-7f95-7c22-8d98-3f15af20c991",
        ),
        (
            RunHistoryRequest::detail(id.clone()),
            "https://target.example/api/runs/018f5e78-7f95-7c22-8d98-3f15af20c991",
        ),
        (
            RunHistoryRequest::page(id, Cursor::new("v2:18446744073709551615")),
            concat!(
                "https://target.example/api/runs/",
                "018f5e78-7f95-7c22-8d98-3f15af20c991/page?after=",
                "v2%3A18446744073709551615"
            ),
        ),
    ];
    for (request, expected) in cases {
        assert_eq!(
            TargetRunHistoryTransport::request_url(&descriptor, &request)
                .assert_value()
                .as_str(),
            expected
        );
    }
}

#[test]
fn request_urls_reject_noncanonical_ids_and_malformed_cursors() {
    let descriptor = descriptor();
    let valid_id = RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991");
    let invalid_id = RunId::new("run-history");
    let mut invalid = vec![
        RunHistoryRequest::list(Some(invalid_id.clone())),
        RunHistoryRequest::detail(invalid_id.clone()),
        RunHistoryRequest::page(invalid_id, Cursor::new("v2:0")),
    ];
    invalid.extend(
        ["v1:1", "v2:", "v2:-1", "v2: 1", "v2:18446744073709551616"]
            .into_iter()
            .map(|cursor| RunHistoryRequest::page(valid_id.clone(), Cursor::new(cursor))),
    );
    for request in invalid {
        assert_eq!(
            TargetRunHistoryTransport::request_url(&descriptor, &request).assert_error(),
            RunHistoryTransportError::Incompatible
        );
    }
    for cursor in ["v2:0", "v2:18446744073709551615"] {
        assert!(valid_cursor(&Cursor::new(cursor)));
    }
}

#[tokio::test]
async fn construction_and_direct_access_fail_closed_without_external_effects() {
    assert!(TargetRunHistoryTransport::new(authority(), target("not a URL")).is_err());

    let transport =
        TargetRunHistoryTransport::new(authority(), target("http://127.0.0.1:8080")).assert_value();
    let state = HistoryDescriptor {
        routes: descriptor(),
        hosted: None,
    };
    assert!(transport.access(&state).await.assert_value().is_none());

    let source = TargetAuthorityError::new("private target detail");
    assert_eq!(incompatible(source), RunHistoryTransportError::Incompatible);
    assert_eq!(
        unavailable(TargetAuthorityError::new("private target detail")),
        RunHistoryTransportError::Unavailable
    );
}
