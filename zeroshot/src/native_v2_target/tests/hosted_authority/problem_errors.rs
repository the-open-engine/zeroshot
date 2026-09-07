use openengine_cluster_testkit::assertions::{AssertError, AssertValue, JsonAt};

use super::direct::{
    spawn_message_only_rejecting_target_authority, spawn_problem_rejecting_target_authority,
};
use super::super::fixtures::{direct_target, exact_run_request, temp_root, test_http_authority};
use super::super::super::{TargetControlAuthority, cli_authority_error};

async fn rejection_diagnostic(origin: String) -> serde_json::Value {
    let root = temp_root();
    let authority = test_http_authority(root.path("refresh-locks"));
    let error = authority
        .submit(&direct_target(origin), &exact_run_request())
        .await
        .assert_error();
    serde_json::to_value(cli_authority_error(error).diagnostic()).assert_value()
}

#[tokio::test]
async fn hosted_problem_details_reach_the_cli_diagnostic() {
    let (origin, server) = spawn_problem_rejecting_target_authority().await;
    let diagnostic = rejection_diagnostic(origin).await;
    assert_eq!(diagnostic.assert_key("kind"), "target");
    assert_eq!(diagnostic.assert_key("code"), "connection_unavailable");
    assert_eq!(
        diagnostic.assert_key("message"),
        "target run request was rejected: required connection \"openrouter\" is unavailable or incomplete"
    );
    assert_eq!(
        diagnostic.assert_key("details").assert_key("connection"),
        "openrouter"
    );
    assert_eq!(
        diagnostic.assert_key("details").assert_key("httpStatus"),
        400
    );
    assert_eq!(
        diagnostic.assert_key("details").assert_key("helpCommand"),
        "zeroshot connection set --help"
    );
    server.await.assert_value();
}

#[tokio::test]
async fn message_only_problem_fails_closed() {
    let (origin, server) = spawn_message_only_rejecting_target_authority().await;
    let diagnostic = rejection_diagnostic(origin).await;
    assert_eq!(diagnostic.assert_key("code"), "invalid_request");
    assert_eq!(
        diagnostic.assert_key("message"),
        "target run request failed with status 400"
    );
    assert_eq!(
        diagnostic.assert_key("details").assert_key("httpStatus"),
        400
    );
    server.await.assert_value();
}
