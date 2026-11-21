//! Registration and identification acceptance for the contributed Forgejo adapter.
use std::collections::BTreeSet;

use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use crate::source_code_provider::adapters::core::{
    forge_credential_field, forge_identity, forge_identity_for,
};
use crate::source_code_provider::*;

use super::provider::ForgejoSourceProvider;
use super::{FORGEJO_CONNECTION, FORGEJO_PROVIDER_ID, FORGEJO_TOKEN_FIELD, register};

const BASE_URL: &str = "https://forge.example";
const REMOTE: &str = "https://forge.example/owner/name.git";
const REPOSITORY: &str = "owner/name";

fn provider() -> ForgejoSourceProvider {
    ForgejoSourceProvider::new(None).assert_value()
}

fn provider_reference() -> SourceProviderRef {
    SourceProviderRef::new(SourceProviderId::new(FORGEJO_PROVIDER_ID).assert_value(), 1)
        .assert_value()
}

fn identified(remote: &str, base: &str) -> Option<String> {
    provider()
        .repository_from_remote_at(remote, base)
        .map(|repository| repository.as_str().to_owned())
}

#[test]
fn f1_descriptor_registers_the_five_declared_capabilities() {
    let provider = provider();
    let reference = provider_reference();
    assert_eq!(provider.descriptor().provider(), &reference);

    let profile = SourceProfileId::new("default").assert_value();
    let declared = provider.descriptor().profile(&profile).assert_value();
    let capabilities = [
        SourceCapability::Read,
        SourceCapability::Push,
        SourceCapability::PullRequest,
        SourceCapability::Checks,
        SourceCapability::Merge,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(declared.capabilities(), &capabilities);
    assert_eq!(provider.descriptor().profiles().len(), 1);
    for capability in [
        SourceCapability::Branch,
        SourceCapability::Commit,
        SourceCapability::Comment,
        SourceCapability::AutoMerge,
        SourceCapability::MergeQueue,
    ] {
        assert!(
            !declared.supports(capability),
            "{capability:?} is not declared"
        );
    }

    let mut registry = SourceCodeProviderRegistry::new();
    register(&mut registry, None, None).assert_value();
    assert_eq!(
        registry.descriptor(&reference).assert_value(),
        provider.descriptor()
    );
    assert_eq!(
        register(&mut registry, None, None).assert_error(),
        SourceRegistryError::DuplicateRegistration {
            provider: reference,
        }
    );
}

#[test]
fn f2_identifies_the_same_repository_across_locators_on_the_selected_instance() {
    for remote in [
        "https://forge.example/owner/name.git",
        "https://forge.example/owner/name",
        "ssh://git@forge.example:2222/owner/name.git",
        "git@forge.example:owner/name.git",
    ] {
        assert_eq!(
            identified(remote, BASE_URL).as_deref(),
            Some(REPOSITORY),
            "{remote}"
        );
    }
}

#[test]
fn f3_identifies_under_an_installation_path_prefix() {
    assert_eq!(
        identified(
            "https://forge.example/git/owner/name.git",
            "https://forge.example/git"
        )
        .as_deref(),
        Some(REPOSITORY)
    );
}

#[test]
fn f4_refuses_other_hosts_deeper_paths_single_segments_and_invalid_bases() {
    for (remote, base) in [
        ("https://other.example/owner/name.git", BASE_URL),
        ("https://forge.example/owner/group/name.git", BASE_URL),
        ("https://forge.example/name.git", BASE_URL),
        (
            "https://forge.example/owner/name.git",
            "http://forge.example",
        ),
    ] {
        assert_eq!(identified(remote, base), None, "{remote} on {base}");
    }
}

#[test]
fn f5_keeps_default_host_selection_off() {
    let provider = provider();
    assert_eq!(provider.repository_from_remote(REMOTE), None);
    assert_eq!(
        provider.repository_from_remote("git@forge.example:owner/name.git"),
        None
    );
}

#[test]
fn f6_declares_connection_metadata_without_a_delivery_authority() {
    let provider = provider();
    assert_eq!(
        provider.default_forge_connection(),
        Some((FORGEJO_CONNECTION, FORGEJO_TOKEN_FIELD))
    );
    assert_eq!(
        forge_credential_field(FORGEJO_PROVIDER_ID).assert_value(),
        Some(FORGEJO_TOKEN_FIELD)
    );
    assert!(provider.delivery_authority().is_none());
    assert!(provider.comment_authority().is_none());
}

#[test]
fn f7_explicit_instance_identity_selects_the_provider_and_auto_selection_stays_off() {
    let base = openengine_cluster_protocol::SourceForgeBaseUrl::new(BASE_URL).assert_value();
    let remote = openengine_cluster_protocol::GitRemoteUrl::new(REMOTE).assert_value();
    let forge = forge_identity_for(&remote, FORGEJO_PROVIDER_ID, &base).assert_value();
    assert_eq!(forge.provider.as_str(), FORGEJO_PROVIDER_ID);
    assert_eq!(forge.repository.as_str(), REPOSITORY);
    assert_eq!(forge.base_url.as_ref(), Some(&base));
    assert!(forge_identity(&remote).assert_value().is_none());
}
