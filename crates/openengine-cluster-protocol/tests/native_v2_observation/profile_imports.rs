use openengine_cluster_protocol::{
    TargetAuthentication, TargetDiscoveryDocument, TargetDiscoveryExtensions,
    SAVED_PROFILE_IMPORTS_KIND,
};
use serde::Deserialize;
use serde_json::json;

#[test]
fn workspace_discovery_is_optional_and_has_a_strict_independent_marker() {
    for authentication in [
        TargetAuthentication::None,
        TargetAuthentication::HostedOauth,
    ] {
        let original = TargetDiscoveryDocument::direct(authentication);
        assert!(original.extensions.run_workspaces.is_none());
        let advertised = original.clone().with_run_workspaces();
        assert_eq!(advertised.extensions.files, original.extensions.files);
        assert_eq!(
            advertised.extensions.saved_profile_imports,
            original.extensions.saved_profile_imports
        );
        assert_eq!(
            serde_json::to_value(advertised).unwrap()["extensions"]["run_workspaces"],
            json!({"kind":openengine_cluster_protocol::RUN_WORKSPACES_KIND})
        );
    }
    for marker in [
        json!({}),
        json!({"kind":true}),
        json!({"kind":openengine_cluster_protocol::RUN_WORKSPACES_KIND,"extra":true}),
    ] {
        assert!(
            serde_json::from_value::<TargetDiscoveryExtensions>(json!({"run_workspaces":marker}))
                .is_err()
        );
    }
}

#[test]
fn saved_profile_imports_is_an_optional_independent_strict_marker() {
    for authentication in [
        TargetAuthentication::None,
        TargetAuthentication::HostedOauth,
    ] {
        let original = TargetDiscoveryDocument::direct(authentication);
        assert!(original.extensions.saved_profile_imports.is_none());
        let advertised = original.clone().with_saved_profile_imports();
        let wire = serde_json::to_value(&advertised).unwrap();
        assert_eq!(
            wire["extensions"]["saved_profile_imports"],
            json!({"kind":SAVED_PROFILE_IMPORTS_KIND})
        );
        assert!(!advertised.extensions.is_empty());
        assert_eq!(advertised.kind, original.kind);
        assert_eq!(
            advertised.extensions.run_profiles,
            original.extensions.run_profiles
        );
        assert_eq!(
            serde_json::from_value::<TargetDiscoveryDocument>(wire).unwrap(),
            advertised
        );
    }
    let only_marker: TargetDiscoveryExtensions = serde_json::from_value(
        json!({"saved_profile_imports":{"kind":SAVED_PROFILE_IMPORTS_KIND}}),
    )
    .unwrap();
    assert!(!only_marker.is_empty());
    for marker in [
        json!({}),
        json!({"kind":SAVED_PROFILE_IMPORTS_KIND,"enabled":true}),
    ] {
        assert!(
            serde_json::from_value::<TargetDiscoveryExtensions>(
                json!({"saved_profile_imports":marker})
            )
            .is_err()
        );
    }
    let future: TargetDiscoveryExtensions = serde_json::from_value(
        json!({"saved_profile_imports":{"kind":"openengine.saved-profile-imports/v2"}}),
    )
    .unwrap();
    assert_ne!(
        future.saved_profile_imports.unwrap().kind,
        SAVED_PROFILE_IMPORTS_KIND
    );
    #[derive(Deserialize)]
    struct LegacyExtensions {
        #[serde(default)]
        run_profiles: Option<openengine_cluster_protocol::TargetRunProfilesDiscovery>,
    }
    let legacy: LegacyExtensions = serde_json::from_value(
        json!({"saved_profile_imports":{"kind":SAVED_PROFILE_IMPORTS_KIND}}),
    )
    .unwrap();
    assert!(legacy.run_profiles.is_none());
}
