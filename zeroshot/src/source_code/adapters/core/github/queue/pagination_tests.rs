//! Required-check pagination has an explicit cursor independent of queue ordering evidence.

use std::sync::Mutex;

use crate::source_code_provider::adapters::core::http_fixture::{IntoResponse, Json};
use reqwest::Method;

use super::*;
use super::super::merge_tests::http_authority;

fn pagination_page(second: bool, truncated_queue: bool) -> Value {
    let mut page = super::recovery_tests::recovery_policy(true);
    let entries = &mut page["data"]["repository"]["mergeQueue"]["entries"];
    entries["totalCount"] = json!(if truncated_queue { 2 } else { 1 });
    entries["queuePageInfo"] = json!({
        "hasNextPage":truncated_queue,"endCursor":"queue-next"
    });
    page["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"] = json!({"contexts":{
        "nodes":[check_run(if second {"second-required"} else {"first-required"}, "COMPLETED", Some("SUCCESS"), true)],
        "pageInfo":{"hasNextPage":!second,"endCursor":if second {"checks-end"} else {"checks-next"}}
    }});
    page
}

#[tokio::test]
async fn github_policy_request_paginates_required_checks_independently_of_queue_entries() {
    for truncated_queue in [false, true] {
        let root = tempfile::tempdir().assert_value();
        let cursors = Arc::new(Mutex::new(Vec::new()));
        let authority = http_authority(root.path(), {
            let cursors = cursors.clone();
            move |request| {
                if request.method == Method::GET {
                    return Json(json!([])).into_response();
                }
                assert_eq!(request.uri.path(), "/graphql");
                let body: Value = serde_json::from_slice(&request.body).assert_value();
                let cursor = body["variables"].get("endCursor").assert_value();
                let mut seen = cursors.lock().unwrap();
                let second = !seen.is_empty();
                seen.push(cursor.clone());
                Json(pagination_page(second, truncated_queue)).into_response()
            }
        });
        let policy = authority
            .policy_snapshot(&review(), SourceDeliveryCredential::token("test-token"))
            .await
            .assert_value();
        assert_eq!(
            *cursors.lock().unwrap(),
            vec![Value::Null, json!("checks-next")]
        );
        assert!(matches!(
            policy.state,
            SourceReviewState::Open {
                checks: SourceChecks::Passed
            }
        ));
        assert_eq!(policy.queue_recovery.is_none(), truncated_queue);
    }
}
