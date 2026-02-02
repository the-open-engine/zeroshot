use std::fs;
use std::path::PathBuf;

use serde::de::DeserializeOwned;
use serde::Serialize;

use zeroshot_tui::protocol::*;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .join("tests/fixtures/tui-v2/protocol")
}

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(path: &PathBuf) {
    let raw = fs::read_to_string(path).expect("read fixture");
    let parsed: T = serde_json::from_str(&raw).expect("deserialize fixture");
    let serialized = serde_json::to_string(&parsed).expect("serialize fixture");
    let reparsed: T = serde_json::from_str(&serialized).expect("re-deserialize fixture");
    assert_eq!(parsed, reparsed, "round-trip mismatch for {:?}", path);
}

#[test]
fn protocol_fixtures_round_trip() {
    let dir = fixtures_dir();
    let entries = fs::read_dir(&dir).expect("read fixtures dir");

    for entry in entries {
        let entry = entry.expect("read entry");
        let path = entry.path();
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();

        if !file_name.ends_with(".json") {
            continue;
        }
        if file_name.starts_with("invalid.") {
            continue;
        }

        let parts: Vec<&str> = file_name.split('.').collect();
        if parts.len() < 3 {
            continue;
        }

        match parts[0] {
            "request" => match parts[1] {
                "initialize" => round_trip::<InitializeRequest>(&path),
                "listClusters" => round_trip::<ListClustersRequest>(&path),
                "getClusterSummary" => round_trip::<GetClusterSummaryRequest>(&path),
                "listClusterMetrics" => round_trip::<ListClusterMetricsRequest>(&path),
                "startClusterFromText" => round_trip::<StartClusterFromTextRequest>(&path),
                "startClusterFromIssue" => round_trip::<StartClusterFromIssueRequest>(&path),
                "sendGuidanceToAgent" => round_trip::<SendGuidanceToAgentRequest>(&path),
                "sendGuidanceToCluster" => round_trip::<SendGuidanceToClusterRequest>(&path),
                "subscribeClusterLogs" => round_trip::<SubscribeClusterLogsRequest>(&path),
                "subscribeClusterTimeline" => round_trip::<SubscribeClusterTimelineRequest>(&path),
                "unsubscribe" => round_trip::<UnsubscribeRequest>(&path),
                "getClusterTopology" => round_trip::<GetClusterTopologyRequest>(&path),
                other => panic!("unknown request fixture: {other}"),
            },
            "response" => match parts[1] {
                "initialize" => round_trip::<InitializeResponse>(&path),
                "listClusters" => round_trip::<ListClustersResponse>(&path),
                "getClusterSummary" => round_trip::<GetClusterSummaryResponse>(&path),
                "listClusterMetrics" => round_trip::<ListClusterMetricsResponse>(&path),
                "startClusterFromText" => round_trip::<StartClusterFromTextResponse>(&path),
                "startClusterFromIssue" => round_trip::<StartClusterFromIssueResponse>(&path),
                "sendGuidanceToAgent" => round_trip::<SendGuidanceToAgentResponse>(&path),
                "sendGuidanceToCluster" => round_trip::<SendGuidanceToClusterResponse>(&path),
                "subscribeClusterLogs" => round_trip::<SubscribeClusterLogsResponse>(&path),
                "subscribeClusterTimeline" => round_trip::<SubscribeClusterTimelineResponse>(&path),
                "unsubscribe" => round_trip::<UnsubscribeResponse>(&path),
                "getClusterTopology" => round_trip::<GetClusterTopologyResponse>(&path),
                other => panic!("unknown response fixture: {other}"),
            },
            "notification" => match parts[1] {
                "clusterLogLines" => round_trip::<ClusterLogLinesNotification>(&path),
                "clusterTimelineEvents" => round_trip::<ClusterTimelineEventsNotification>(&path),
                other => panic!("unknown notification fixture: {other}"),
            },
            other => panic!("unknown fixture prefix: {other}"),
        }
    }
}
