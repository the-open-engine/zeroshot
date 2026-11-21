//! Delivery-authority acceptance for the contributed Forgejo adapter. Every read is observed through
//! the loopback fixture, so the assertions cover what the authority sends as well as what it returns.
use std::path::Path;

use openengine_cluster_testkit::assertions::{AssertAt, AssertError, AssertValue};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use openengine_cluster_protocol::SourceForgeBaseUrl;

use crate::source_code_provider::{
    DeliveryTarget, SourceDeliveryAuthority, SourceDeliveryCredential, SourceHeadReconciliation,
    SourcePushRequest, SourceReviewReceipt, SourceTargetReconciliation,
};

use super::api::{Api, ApiRequest, PAGE_SIZE};
use super::authority::{split_repository, ForgejoDeliveryAuthority};
use super::http_fixture::{Fixture, Response};

const TOKEN: &str = "forgejo-test-token";
const BASE: &str = "https://forge.example";
const REPOSITORY: &str = "owner/name";
const REVISION: &str = "0123456789012345678901234567890123456789";

fn credential() -> SourceDeliveryCredential<'static> {
    SourceDeliveryCredential::token(TOKEN)
}

async fn authority(fixture: &Fixture) -> ForgejoDeliveryAuthority {
    ForgejoDeliveryAuthority::for_test(
        SourceForgeBaseUrl::new(BASE).assert_value(),
        Api::loopback(&fixture.endpoint()).assert_value(),
    )
}

fn version() -> Response {
    Response::json(json!({"version": "16.0.0"}))
}

fn user(login: &str) -> Response {
    Response::json(json!({"id": 7, "login": login, "unknown_field": "ignored"}))
}

fn repository_body(permissions: Option<Value>) -> Value {
    let mut body = json!({
        "full_name": REPOSITORY,
        "html_url": format!("{BASE}/{REPOSITORY}"),
        "archived": false,
        "unknown_field": "ignored",
    });
    if let Some(permissions) = permissions {
        body["permissions"] = permissions;
    }
    body
}

#[tokio::test]
async fn c1_reads_the_user_the_version_and_the_repository_and_sends_only_reads() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    fixture
        .queue(user("zeroshot"))
        .queue(version())
        .queue(Response::json(repository_body(Some(
            json!({"pull": true, "push": true}),
        ))));

    authority
        .check_access(REPOSITORY, credential(), None)
        .await
        .assert_value();

    let requests = fixture.requests();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(request.method, Method::GET, "{request:?}");
    }
    assert_eq!(requests.assert_at(0).uri, "/api/v1/user");
    assert_eq!(requests.assert_at(1).uri, "/api/v1/version");
    assert_eq!(requests.assert_at(2).uri, "/api/v1/repos/owner/name");
}

#[tokio::test]
async fn c2_an_empty_login_is_an_authentication_failure() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    fixture.queue(user(""));

    let error = authority
        .check_access(REPOSITORY, credential(), None)
        .await
        .assert_error();
    assert_eq!(error.api_status(), Some(401));
    assert_eq!(
        format!("{error}"),
        "source API request failed: Forgejo authentication check failed: token did not identify a user"
    );
    assert_eq!(fixture.requests().len(), 1);
}

#[tokio::test]
async fn c3_a_refused_user_read_keeps_its_status() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    fixture.queue(Response::status(StatusCode::UNAUTHORIZED));

    let error = authority
        .check_access(REPOSITORY, credential(), None)
        .await
        .assert_error();
    assert_eq!(error.api_status(), Some(401));
    assert!(error.authentication_failed(), "{error}");
    assert!(format!("{error}").contains("401"), "{error}");
    assert_eq!(fixture.requests().len(), 1);
}

#[tokio::test]
async fn c4_the_read_must_confirm_the_admitted_repository_identity() {
    let cases = [
        (
            Response::status(StatusCode::NOT_FOUND),
            "Forgejo repository read returned no repository for owner/name",
        ),
        (
            Response::json(json!({
                "full_name": "other/name",
                "html_url": format!("{BASE}/{REPOSITORY}"),
            })),
            "Forgejo repository read returned other/name instead of owner/name",
        ),
        (
            Response::json(json!({
                "full_name": REPOSITORY,
                "html_url": "https://other.example/owner/name",
            })),
            "Forgejo repository read returned https://other.example/owner/name instead of the admitted instance https://forge.example/owner/name",
        ),
        (
            Response::json(json!({
                "full_name": REPOSITORY,
                "html_url": format!("{BASE}/{REPOSITORY}"),
                "permissions": {"pull": true, "push": true},
                "archived": true,
            })),
            "Forgejo repository owner/name is archived",
        ),
    ];
    for (response, expected) in cases {
        let fixture = Fixture::new().await;
        let authority = authority(&fixture).await;
        fixture
            .queue(user("zeroshot"))
            .queue(version())
            .queue(response);

        let error = authority
            .check_access(REPOSITORY, credential(), None)
            .await
            .assert_error();
        assert_eq!(
            format!("{error}"),
            format!("source delivery identity mismatch: {expected}")
        );
    }
}

#[tokio::test]
async fn c5_read_permission_is_required_and_missing_permission_is_not_read_access() {
    for permissions in [None, Some(json!({"pull": false, "push": true}))] {
        let fixture = Fixture::new().await;
        let authority = authority(&fixture).await;
        fixture
            .queue(user("zeroshot"))
            .queue(version())
            .queue(Response::json(repository_body(permissions)));

        let error = authority
            .check_access(REPOSITORY, credential(), None)
            .await
            .assert_error();
        assert_eq!(
            format!("{error}"),
            "source delivery identity mismatch: Forgejo repository read failed: token cannot read this repository"
        );
    }
}

#[tokio::test]
async fn c6_a_server_below_the_floor_is_refused() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    fixture
        .queue(user("zeroshot"))
        .queue(Response::json(json!({"version": "15.0.0"})))
        .queue(Response::json(repository_body(Some(
            json!({"pull": true, "push": true}),
        ))));

    let error = authority
        .check_access(REPOSITORY, credential(), None)
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        "source delivery identity mismatch: Forgejo server 16.0.0 or newer is required for delivery"
    );
    assert_eq!(fixture.requests().len(), 2);
}

#[tokio::test]
async fn c7_a_signalled_stop_ends_the_check_before_any_request() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let (_, stopping) = tokio::sync::watch::channel(true);

    let error = authority
        .check_access(REPOSITORY, credential(), Some(stopping))
        .await
        .assert_error();
    assert_eq!(error.api_status(), None);
    assert_eq!(
        format!("{error}"),
        "source API request failed: Forgejo repository access request was stopped before it completed"
    );
    assert_eq!(fixture.requests().len(), 0);
}

#[tokio::test]
async fn c8_branch_reads_keep_each_name_part_as_one_path_segment() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    fixture
        .queue(version())
        .queue(Response::json(repository_body(Some(
            json!({"pull": true, "push": true}),
        ))));

    fixture.queue(Response::json(json!({
        "name": "feature/x",
        "commit": {"id": REVISION},
    })));
    let revision = authority
        .branch_revision(REPOSITORY, "feature/x", credential())
        .await
        .assert_value();
    assert_eq!(revision.as_deref(), Some(REVISION));
    assert_eq!(
        fixture.requests().assert_at(2).uri,
        "/api/v1/repos/owner/name/branches/feature/x"
    );

    fixture.queue(Response::json(repository_body(Some(
        json!({"pull": true, "push": true}),
    ))));
    fixture.queue(Response::status(StatusCode::NOT_FOUND));
    assert_eq!(
        authority
            .branch_revision(REPOSITORY, "feature/y", credential())
            .await
            .assert_value(),
        None
    );

    fixture.queue(Response::json(repository_body(Some(
        json!({"pull": true, "push": true}),
    ))));
    fixture.queue(Response::json(json!({
        "name": "feature/z",
        "commit": {"id": REVISION},
    })));
    let error = authority
        .branch_revision(REPOSITORY, "feature/x", credential())
        .await
        .assert_error();
    assert!(
        format!("{error}").contains("Forgejo branch read returned feature/z instead of feature/x"),
        "{error}"
    );

    fixture.queue(Response::json(repository_body(Some(
        json!({"pull": true, "push": true}),
    ))));
    fixture.queue(Response::json(json!({
        "name": "feature/x",
        "commit": {"id": "not-a-revision"},
    })));
    let error = authority
        .branch_revision(REPOSITORY, "feature/x", credential())
        .await
        .assert_error();
    assert!(
        format!("{error}")
            .contains("Forgejo branch feature/x returned <invalid revision> as its revision"),
        "{error}"
    );

    // An absent target branch is an identity error naming the branch and the repository, and the
    // version read is not repeated for it.
    fixture.queue(Response::json(repository_body(Some(
        json!({"pull": true, "push": true}),
    ))));
    fixture.queue(Response::status(StatusCode::NOT_FOUND));
    let target = DeliveryTarget::new(REPOSITORY, "main", REVISION).assert_value();
    let error = authority
        .reconcile_delivery_target(
            SourceTargetReconciliation {
                workspace: Path::new("."),
                target: &target,
                commit_message: "Delivery",
            },
            credential(),
        )
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        "source delivery identity mismatch: Forgejo target branch is absent: branch main in repository owner/name (admitted source revision 0123456789012345678901234567890123456789). Restore the target branch before resuming."
    );
    assert_eq!(fixture.requests().len(), 11);
}

#[tokio::test]
async fn c9_repository_identity_is_exactly_owner_slash_name() {
    for value in ["owner", "owner/", "/name", "a/b/c", "", "owner/name/extra"] {
        let error = split_repository(value).assert_error();
        assert_eq!(
            format!("{error}"),
            "source delivery identity mismatch: Forgejo repository identity must be <owner>/<name>",
            "{value}"
        );
    }
    assert_eq!(
        split_repository("owner/name").assert_value(),
        ("owner", "name")
    );
}

#[tokio::test]
async fn c10_delivery_needs_the_admitted_transport_and_no_diagnostic_shows_the_token() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    let debug = format!("{authority:?}");
    assert!(!debug.contains(TOKEN), "{debug}");
    assert!(debug.contains("Api([REDACTED])"), "{debug}");

    let target = DeliveryTarget::new(REPOSITORY, "main", REVISION).assert_value();
    let published = SourceReviewReceipt {
        review_id: "17".to_owned(),
        repository: REPOSITORY.to_owned(),
        target_branch: "main".to_owned(),
        head_branch: "zeroshot/test".to_owned(),
        head_revision: REVISION.to_owned(),
    };
    let error = authority
        .reconcile_delivery_head(
            SourceHeadReconciliation {
                require_fast_forward: true,
                workspace: Path::new("."),
                published: &published,
                observed: &published,
                commit_message: "Delivery",
                authorized_update: false,
                adopting_existing: false,
            },
            credential(),
        )
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        "source delivery identity mismatch: Forgejo Git delivery requires the admitted Git transport"
    );
    assert!(!format!("{error}").contains(TOKEN), "{error}");

    // A push is a transport effect, so the read happens first and the effect stays delegated.
    fixture
        .queue(version())
        .queue(Response::json(repository_body(Some(
            json!({"pull": true, "push": true}),
        ))));
    let error = authority
        .push_branch(
            &SourcePushRequest {
                workspace: Path::new(".").to_path_buf(),
                target: target.clone(),
                head_branch: "zeroshot/test".to_owned(),
                head_revision: REVISION.to_owned(),
            },
            credential(),
        )
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        "source delivery identity mismatch: Forgejo Git delivery requires the admitted Git transport"
    );
}

#[tokio::test]
async fn c11_read_permission_without_write_permission_is_refused_before_any_effect() {
    let fixture = Fixture::new().await;
    let authority = authority(&fixture).await;
    fixture
        .queue(user("zeroshot"))
        .queue(version())
        .queue(Response::json(repository_body(Some(
            json!({"pull": true, "push": false}),
        ))));

    let error = authority
        .check_access(REPOSITORY, credential(), None)
        .await
        .assert_error();
    assert_eq!(
        format!("{error}"),
        "source delivery identity mismatch: Forgejo repository write failed: token cannot push to this repository"
    );
    // The check observes only, so the refused token never reaches a write.
    let requests = fixture.requests();
    assert_eq!(requests.len(), 3);
    for request in &requests {
        assert_eq!(request.method, Method::GET, "{request:?}");
    }
}

#[tokio::test]
async fn a11_an_empty_page_before_the_reported_total_is_an_error() {
    let fixture = Fixture::new().await;
    let api = Api::loopback(&fixture.endpoint()).assert_value();
    let path = ["repos", "owner", "name"];

    // A page that already holds the reported total is the end of the read.
    fixture.queue(Response::json(vec![json!(1), json!(2)]).header("x-total-count", "2"));
    let collected: Vec<Value> = api
        .pages(ApiRequest::get(&path), credential())
        .await
        .assert_value();
    assert_eq!(collected, vec![json!(1), json!(2)]);
    assert_eq!(fixture.requests().len(), 1);

    // A shortened list could hide an existing pull request, so it is not the end of the read.
    fixture
        .queue(Response::json(vec![json!(1)]).header("x-total-count", "120"))
        .queue(Response::json(Vec::<Value>::new()).header("x-total-count", "120"));
    let error = api
        .pages::<Value>(ApiRequest::get(&path), credential())
        .await
        .assert_error();
    assert!(
        format!("{error}")
            .contains("Forgejo pagination returned an empty page before the reported total of 120"),
        "{error}"
    );
    let requests = fixture.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests.assert_at(2).uri,
        format!("/api/v1/repos/owner/name?limit={PAGE_SIZE}&page=2")
    );

    // A genuinely empty collection is still an answer when the total says so.
    fixture.queue(Response::json(Vec::<Value>::new()).header("x-total-count", "0"));
    let collected: Vec<Value> = api
        .pages(ApiRequest::get(&path), credential())
        .await
        .assert_value();
    assert!(collected.is_empty());
    assert_eq!(fixture.requests().len(), 4);
}
