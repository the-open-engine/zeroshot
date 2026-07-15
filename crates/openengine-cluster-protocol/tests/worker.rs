use openengine_cluster_protocol::{
    legacy_ship_request_payload_type, legacy_ship_result_payload_type, GraphProfile,
    LegacyShipRequest, WorkerDescriptor, WorkerFailureReason, WorkerOutcome, WorkerProtocolBinding,
    ACP_PROFILE, ACP_VERSION, LEGACY_ZEROSHOT_WORKER, RUNTIME_WORKER_ERRORS,
};
use serde_json::json;

fn descriptor() -> serde_json::Value {
    json!({
        "worker": "mock.acp@1",
        "graphProfiles": ["openengine.graph.full/v1"],
        "binding": { "protocol": "acp", "version": ACP_VERSION, "profile": ACP_PROFILE },
        "contract": {
            "input": { "kind": "string" },
            "output": { "kind": "string" },
            "verifier": null,
            "errors": ["timeout", "crash", "malformed", "refusal"]
        },
        "capabilityPolicy": {
            "autonomy": "strict",
            "permissionPolicy": "policy.strict@1"
        },
        "artifactProfile": {
            "allowedTypeIds": ["openengine.result@1"],
            "allowedMediaTypes": ["application/json"],
            "minimumRedaction": "internal"
        },
        "credentialRequirements": ["credential.mock@1"]
    })
}

#[test]
fn bindings_are_exact_and_descriptor_fields_are_closed() {
    assert!(serde_json::from_value::<WorkerDescriptor>(descriptor()).is_ok());
    let schema = serde_json::to_value(schemars::schema_for!(WorkerDescriptor)).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&descriptor()));

    for (field, value) in [
        ("command", json!("curl example")),
        ("endpoint", json!("https://example.invalid")),
        ("token", json!("secret")),
        ("credentialValue", json!("secret")),
        ("callback", json!("ask-user")),
        ("path", json!("/tmp/secret")),
    ] {
        let mut rejected = descriptor();
        rejected[field] = value;
        assert!(serde_json::from_value::<WorkerDescriptor>(rejected.clone()).is_err());
        assert!(!validator.is_valid(&rejected));
    }

    let mut unsupported = descriptor();
    unsupported["binding"]["version"] = json!("2");
    assert!(serde_json::from_value::<WorkerDescriptor>(unsupported.clone()).is_err());
    assert!(!validator.is_valid(&unsupported));
    assert_eq!(WorkerProtocolBinding::acp_v1().version, ACP_VERSION);
}

#[test]
fn descriptor_rejects_empty_duplicate_sets_and_nonopaque_handles() {
    for pointer in [
        "/graphProfiles",
        "/contract/errors",
        "/artifactProfile/allowedTypeIds",
    ] {
        let mut rejected = descriptor();
        *rejected.pointer_mut(pointer).unwrap() = json!([]);
        assert!(serde_json::from_value::<WorkerDescriptor>(rejected).is_err());
    }
    let mut duplicate = descriptor();
    duplicate["graphProfiles"] = json!(["openengine.graph.full/v1", "openengine.graph.full/v1"]);
    assert!(serde_json::from_value::<WorkerDescriptor>(duplicate).is_err());

    let mut incomplete_errors = descriptor();
    incomplete_errors["contract"]["errors"] = json!(["timeout", "crash", "malformed"]);
    assert!(serde_json::from_value::<WorkerDescriptor>(incomplete_errors.clone()).is_err());
    let schema = serde_json::to_value(schemars::schema_for!(WorkerDescriptor)).unwrap();
    assert!(
        !jsonschema::validator_for(&schema)
            .unwrap()
            .is_valid(&incomplete_errors)
    );

    for handle in ["raw-token", "env/API_TOKEN", "https://credentials.invalid"] {
        let mut rejected = descriptor();
        rejected["credentialRequirements"] = json!([handle]);
        assert!(serde_json::from_value::<WorkerDescriptor>(rejected).is_err());
    }
}

#[test]
fn strict_autonomy_has_only_typed_fail_closed_outcomes() {
    for (outcome, reason) in [
        (
            WorkerOutcome::refusal(WorkerFailureReason::PolicyDenied),
            "policy_denied",
        ),
        (
            WorkerOutcome::refusal(WorkerFailureReason::InteractiveInputRequired),
            "interactive_input_required",
        ),
        (
            WorkerOutcome::refusal(WorkerFailureReason::AuthenticationRequired),
            "authentication_required",
        ),
    ] {
        let value = serde_json::to_value(outcome).unwrap();
        assert_eq!(value["status"], "error");
        assert_eq!(value["code"], "refusal");
        assert_eq!(value["reason"], reason);
        assert!(value.get("callback").is_none());
    }
    assert_eq!(
        serde_json::to_value(WorkerOutcome::malformed()).unwrap()["code"],
        "malformed"
    );
}

#[test]
fn legacy_ship_contract_is_single_worker_and_source_consistent() {
    let mut legacy = descriptor();
    legacy["worker"] = json!(LEGACY_ZEROSHOT_WORKER);
    legacy["graphProfiles"] = json!([GraphProfile::SingleWorker.as_str()]);
    legacy["binding"] =
        serde_json::to_value(WorkerProtocolBinding::legacy_zeroshot_ship_v1()).unwrap();
    legacy["contract"]["input"] = serde_json::to_value(legacy_ship_request_payload_type()).unwrap();
    legacy["contract"]["output"] = serde_json::to_value(legacy_ship_result_payload_type()).unwrap();
    assert!(serde_json::from_value::<WorkerDescriptor>(legacy.clone()).is_ok());
    for pointer in ["/contract/input", "/contract/output"] {
        let mut invalid = legacy.clone();
        *invalid.pointer_mut(pointer).unwrap() = json!({ "kind": "string" });
        assert!(serde_json::from_value::<WorkerDescriptor>(invalid).is_err());
    }
    let mut invalid = legacy.clone();
    invalid["contract"]["errors"] = json!(["crash", "timeout", "malformed", "refusal"]);
    assert!(serde_json::from_value::<WorkerDescriptor>(invalid).is_err());
    let mut invalid = legacy;
    invalid["graphProfiles"] = json!([GraphProfile::Full.as_str()]);
    assert!(serde_json::from_value::<WorkerDescriptor>(invalid).is_err());

    let base = json!({
        "source": "issue",
        "issue": "649",
        "prompt": null,
        "artifacts": [],
        "isolationProfile": "isolation.worktree@1",
        "providerProfile": "provider.default@1"
    });
    assert!(serde_json::from_value::<LegacyShipRequest>(base.clone()).is_ok());
    let mut inconsistent = base;
    inconsistent["prompt"] = json!("also prompt");
    assert!(serde_json::from_value::<LegacyShipRequest>(inconsistent.clone()).is_err());
    let schema = serde_json::to_value(schemars::schema_for!(LegacyShipRequest)).unwrap();
    assert!(
        !jsonschema::validator_for(&schema)
            .unwrap()
            .is_valid(&inconsistent)
    );

    let errors = serde_json::from_value::<WorkerDescriptor>(descriptor())
        .unwrap()
        .contract
        .errors;
    assert_eq!(errors, RUNTIME_WORKER_ERRORS);
}
