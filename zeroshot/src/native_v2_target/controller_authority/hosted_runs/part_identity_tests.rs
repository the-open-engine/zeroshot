use super::*;
use openengine_cluster_protocol::RunResumeResult;
use serde_json::json;

#[test]
fn hosted_guards_survive_transport_and_legacy_adaptation() {
    let root = RunId::new("root");
    let child = RunId::new("child");
    let address = Url::parse("https://example.test/logs?execution=node").unwrap();
    let selected = selected_url(address.clone(), true, &root, Some(&child)).unwrap();
    assert_eq!(selected.query(), Some("execution=node&partId=child"));
    assert!(selected_url(address, false, &root, Some(&child)).is_err());

    let mut body = json!({"runId":root,"partId":child,"successorPartId":"next"});
    assert_eq!(request(true, &body).unwrap(), body);
    assert!(request(false, &body).is_err());
    body["partId"] = json!(root);
    assert_eq!(
        request(false, &body).unwrap(),
        json!({"runId":root,"successorRunId":"next"})
    );

    let old = br#"{"runId":"next","resumedFrom":"root"}"#;
    let result: RunResumeResult = decode(old, false, |v| {
        physical_runs::read_response(openengine_cluster_protocol::RUN_RESUME_METHOD, v)
    })
    .unwrap();
    assert_eq!(result.current_part_id.as_str(), "next");
    assert_eq!(result.resumed_from, root);
    assert!(decode::<RunResumeResult>(old, true, |_| {}).is_err());
}
