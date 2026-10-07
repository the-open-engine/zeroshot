use super::*;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

#[test]
fn merge_plan_environment_is_optional_concrete_and_selects_declared_hook_credentials() {
    let root = tempfile::tempdir().assert_value();
    let file = root.path().join("plan.json");
    let mut document = test_manifest_json();
    let tomorrow = OffsetDateTime::now_utc() + TimeDuration::days(1);
    document["expiresAt"] = json!(format!(
        "{:04}-{:02}-{:02}T00:00:00Z",
        tomorrow.year(),
        tomorrow.month() as u8,
        tomorrow.day()
    ));
    let runtime = serde_json::from_value(
        json!({"harness":"codex", "provider":"openai", "size":"small", "nodes":{}}),
    )
    .assert_value();
    let profile = RunProfile {
        id: "profile".into(),
        name: RunProfileName::new("plain").assert_value(),
        scope: RunProfileScope::Org,
        graph: crate::native_v2_cli::BuiltinGraphTemplate::SingleWorker
            .materialize(crate::native_v2_cli::TemplateDelivery::None)
            .assert_value(),
        runtime,
        is_default: false,
    };
    for selection in [
        None,
        Some(json!({})),
        Some(json!({"startup":"npm ci", "connections":{"registry":["NPM_TOKEN"]}})),
    ] {
        if let Some(definition) = &selection {
            document["environment"] = definition.clone();
        }
        std::fs::write(&file, serde_json::to_vec(&document).assert_value()).assert_value();
        let manifest = load_manifest(&file).assert_value();
        assert_eq!(
            serde_json::to_value(&manifest.environment).assert_value(),
            selection.clone().unwrap_or_default()
        );
        let request = prepared_request(
            IdempotencyKey::new("plan").assert_value(),
            manifest,
            &profile,
            &|name| (name == "NPM_TOKEN").then(|| OsString::from("registry-secret")),
        )
        .assert_value();
        assert_eq!(
            request.connections.len(),
            usize::from(
                selection
                    .as_ref()
                    .is_some_and(|definition| definition.get("connections").is_some())
            )
        );
        assert_eq!(
            serde_json::to_value(request.environment).assert_value(),
            selection.unwrap_or_default()
        );
    }
    document["environment"] = json!({"variables":{"PATH":"override"}});
    std::fs::write(&file, serde_json::to_vec(&document).assert_value()).assert_value();
    assert!(load_manifest(&file).is_err());
}
