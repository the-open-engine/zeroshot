use openengine_cluster_protocol::RunProfileListResult;
use serde_json::json;

#[test]
fn target_v1_profile_listing_retains_its_original_wire_and_rust_contract() {
    let listed = RunProfileListResult {
        profiles: Vec::new(),
    };
    let legacy = json!({"profiles": []});
    assert_eq!(serde_json::to_value(listed).unwrap(), legacy);
    assert!(serde_json::from_value::<RunProfileListResult>(legacy).is_ok());
    assert!(
        serde_json::from_value::<RunProfileListResult>(json!({
            "profiles": [], "problems": [{"name":"broken", "scope":"user", "isDefault":true,
                "message":"profile broken has malformed metadata"}]
        }))
        .is_err(),
        "v1 clients reject unnegotiated fields"
    );
}
