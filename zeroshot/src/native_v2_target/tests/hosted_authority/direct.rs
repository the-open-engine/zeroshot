use openengine_cluster_protocol::{
    RUN_HISTORY_KIND, TargetAuthentication, TargetDiscoveryDocument, TargetRunHistoryDiscovery,
    TargetRunHistoryRoutes,
};
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;
use tokio::net::TcpListener;

use super::{CapturedHttpRequest, read_http_request, write_http_response_with_status};

enum RunResponse {
    Accepted,
    Rejected,
    RejectedConnection,
    RejectedMessageOnly,
}

pub(in crate::native_v2_target::tests) async fn spawn_direct_target_authority(
    request_count: usize,
) -> (String, tokio::task::JoinHandle<Vec<CapturedHttpRequest>>) {
    spawn_direct_target_authority_with_response(request_count, RunResponse::Accepted).await
}

pub(in crate::native_v2_target::tests) async fn spawn_rejecting_direct_target_authority()
-> (String, tokio::task::JoinHandle<()>) {
    let (origin, server) =
        spawn_direct_target_authority_with_response(2, RunResponse::Rejected).await;
    (
        origin,
        tokio::spawn(async move { drop(server.await.assert_value()) }),
    )
}

pub(super) async fn spawn_problem_rejecting_target_authority()
-> (String, tokio::task::JoinHandle<()>) {
    let (origin, server) =
        spawn_direct_target_authority_with_response(2, RunResponse::RejectedConnection).await;
    (
        origin,
        tokio::spawn(async move { drop(server.await.assert_value()) }),
    )
}

pub(super) async fn spawn_message_only_rejecting_target_authority()
-> (String, tokio::task::JoinHandle<()>) {
    let (origin, server) =
        spawn_direct_target_authority_with_response(2, RunResponse::RejectedMessageOnly).await;
    (
        origin,
        tokio::spawn(async move { drop(server.await.assert_value()) }),
    )
}

async fn spawn_direct_target_authority_with_response(
    request_count: usize,
    run_response: RunResponse,
) -> (String, tokio::task::JoinHandle<Vec<CapturedHttpRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.assert_value();
    let address = listener.local_addr().assert_value();
    let origin = format!("http://{address}");
    let server = tokio::spawn(async move {
        let mut captured = Vec::new();
        for _ in 0..request_count {
            let (mut stream, _) = listener.accept().await.assert_value();
            let request = read_http_request(&mut stream).await;
            let (status, body) = response(&request, address, &run_response);
            write_http_response_with_status(&mut stream, status, &body).await;
            captured.push(request);
        }
        captured
    });
    (origin, server)
}

fn response(
    request: &CapturedHttpRequest,
    address: std::net::SocketAddr,
    run_response: &RunResponse,
) -> (&'static str, String) {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/.well-known/zeroshot-native-v2") => ("200 OK", discovery(address)),
        ("GET", path) if path.starts_with("/direct-history") => (
            "200 OK",
            json!({"runs": [], "nextCursor": null}).to_string(),
        ),
        ("POST", "/native-v2/run") => run_submission_response(run_response),
        ("POST", "/native-v2/oecp-session") => (
            "200 OK",
            format!(r#"{{"endpoint":"ws://{address}/native-v2/oecp"}}"#),
        ),
        unexpected => None::<(&str, String)>
            .assert_value_with(&format!("unexpected direct request: {unexpected:?}")),
    }
}

fn discovery(address: std::net::SocketAddr) -> String {
    serde_json::to_string(
        &TargetDiscoveryDocument::direct(TargetAuthentication::None)
            .with_workspace_recovery()
            .with_workspace_checkpoints()
            .with_run_history(TargetRunHistoryDiscovery {
                kind: RUN_HISTORY_KIND.to_owned(),
                base_url: format!("http://{address}"),
                route_templates: TargetRunHistoryRoutes {
                    list: "/direct-history{?after}".to_owned(),
                    detail: "/direct-history/{run_id}".to_owned(),
                    page: "/direct-history/{run_id}/page{?after}".to_owned(),
                },
            }),
    )
    .assert_value()
}

fn run_submission_response(response: &RunResponse) -> (&'static str, String) {
    match response {
        RunResponse::Accepted => (
            "200 OK",
            r#"{"runId":"018f5e78-7f95-7c22-8d98-3f15af20c991"}"#.to_owned(),
        ),
        RunResponse::Rejected => (
            "400 Bad Request",
            json!({
                "code": "run.rejected",
                "message": "required payload target issueNumber is not defined by a binding"
            })
            .to_string(),
        ),
        RunResponse::RejectedConnection => (
            "400 Bad Request",
            json!({
                "code": "connection_unavailable",
                "message": "required connection \"openrouter\" is unavailable or incomplete",
                "details": {"connection": "openrouter"}
            })
            .to_string(),
        ),
        RunResponse::RejectedMessageOnly => (
            "400 Bad Request",
            r#"{"message":"legacy rejection"}"#.to_owned(),
        ),
    }
}
