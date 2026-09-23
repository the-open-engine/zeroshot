use std::collections::{BTreeMap, BTreeSet};

use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde::{Deserialize, Serialize};

provider_contract_types!(
    TestError,
    TestProviderId,
    TestProfileId,
    TestAccountId,
    TestCredentialId,
    TestOperationId,
    TestFingerprint,
    TestProviderRef,
    8,
    "test"
);
bounded_bytes_type!(TestBytes, 4, TestError, "bytes");

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum TestCapability {
    Read,
    Write,
}

profile_descriptor_type!(
    TestProfile,
    TestProfileWire,
    "TestProfileWire",
    TestCapability,
    TestError,
    "capabilities"
);
provider_descriptor_type!(
    TestDescriptor,
    TestDescriptorWire,
    "TestDescriptorWire",
    TestProviderRef,
    TestProfileId,
    TestProfile,
    TestError,
    "profiles"
);

#[test]
fn coverage_contract_generated_provider_values_preserve_display_accessors_and_validation_context() {
    let provider = TestProviderId::new("provider.one").assert_value();
    assert_eq!(provider.as_str(), "provider.one");
    assert_eq!(provider.to_string(), "provider.one");

    let profile_id = TestProfileId::new("profile").assert_value();
    let account = TestAccountId::new("account").assert_value();
    let credential = TestCredentialId::new("secret").assert_value();
    let operation = TestOperationId::new("op").assert_value();
    for (actual, expected) in [
        (profile_id.to_string(), "profile"),
        (account.to_string(), "account"),
        (credential.to_string(), "secret"),
        (operation.to_string(), "op"),
    ] {
        assert_eq!(actual, expected);
    }

    let fingerprint = TestFingerprint::new("a".repeat(64)).assert_value();
    assert_eq!(fingerprint.as_str(), "a".repeat(64));
    assert_eq!(fingerprint.to_string(), "a".repeat(64));
    let reference = TestProviderRef::new(provider, 3).assert_value();
    assert_eq!(reference.id().as_str(), "provider.one");
    assert_eq!(reference.version(), 3);
    assert_eq!(reference.to_string(), "provider.one@3");

    let bytes = TestBytes::new("éé").assert_value();
    assert_eq!(bytes.as_str(), "éé");
    assert_eq!(bytes.to_string(), "éé");
    let error = TestBytes::new("five!").assert_error();
    assert_eq!(error.field(), "bytes");
    assert!(error.reason().contains("maximum is 4"));
    assert!(TestProviderRef::new(TestProviderId::new("p").assert_value(), 0).is_err());
}

#[test]
fn coverage_contract_generated_descriptors_enforce_capability_subset_on_build_and_decode() {
    let capabilities = BTreeSet::from([TestCapability::Read, TestCapability::Write]);
    let idempotent = BTreeSet::from([TestCapability::Read]);
    let profile = TestProfile::new(capabilities.clone(), idempotent).assert_value();
    assert_eq!(profile.capabilities(), &capabilities);
    assert!(profile.supports(TestCapability::Read));
    assert!(profile.has_provider_native_idempotency(TestCapability::Read));
    assert!(!profile.has_provider_native_idempotency(TestCapability::Write));

    let invalid = TestProfile::new(
        BTreeSet::from([TestCapability::Read]),
        BTreeSet::from([TestCapability::Write]),
    )
    .assert_error();
    assert_eq!(invalid.field(), "provider-native idempotency");

    let decoded: TestProfile = serde_json::from_value(serde_json::json!({
        "capabilities": ["read", "write"],
        "providerNativeIdempotency": ["read"]
    }))
    .assert_value();
    assert!(decoded.supports(TestCapability::Write));
    assert!(
        serde_json::from_value::<TestProfile>(serde_json::json!({
            "capabilities": ["read"],
            "providerNativeIdempotency": ["write"]
        }))
        .is_err()
    );

    let profile_id = TestProfileId::new("default").assert_value();
    let reference =
        TestProviderRef::new(TestProviderId::new("provider").assert_value(), 1).assert_value();
    let descriptor =
        TestDescriptor::new(reference, BTreeMap::from([(profile_id.clone(), profile)]))
            .assert_value();
    assert_eq!(descriptor.provider().version(), 1);
    assert_eq!(descriptor.profiles().len(), 1);
    assert!(descriptor.profile(&profile_id).is_some());

    let roundtrip: TestDescriptor =
        serde_json::from_value(serde_json::to_value(&descriptor).assert_value()).assert_value();
    assert_eq!(roundtrip, descriptor);
}
