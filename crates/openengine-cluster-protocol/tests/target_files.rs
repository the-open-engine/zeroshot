use openengine_cluster_protocol::*;
use serde::Deserialize;
use serde_json::json;

#[test]
fn file_capability_is_independent_and_legacy_discovery_readers_ignore_it() {
    #[derive(Deserialize)]
    struct LegacyExtensions {
        workspace_recovery: Option<TargetWorkspaceRecoveryDiscovery>,
    }
    let mut document = TargetDiscoveryDocument::direct(TargetAuthentication::None);
    assert!(document.extensions.is_empty());
    document.extensions.files = Some(TargetFilesDiscovery {
        kind: TARGET_FILES_KIND.to_owned(),
    });
    assert!(!document.extensions.is_empty());
    let serialized = serde_json::to_value(document).unwrap();
    let legacy: LegacyExtensions =
        serde_json::from_value(serialized["extensions"].clone()).unwrap();
    assert!(legacy.workspace_recovery.is_none());
    assert!(
        serde_json::from_value::<TargetFilesDiscovery>(
            json!({"kind": TARGET_FILES_KIND, "route": "/other"})
        )
        .is_err()
    );
    assert!(serde_json::from_value::<TargetHostedRunRoutes>(json!({"list":"/list","status":"/status","watch":"/watch","logs":"/logs","force":"/force","files":"/files"})).is_err());
}

#[test]
fn file_requests_are_byte_free_strict_and_validate_portable_members() {
    let selection = json!({"address":{"scope":{"kind":"user"},"path":"datasets/tree"},"member":"nested/file.bin"});
    let request = json!({"action":"download", "selection":selection});
    assert_eq!(
        serde_json::to_value(serde_json::from_value::<FileRequest>(request.clone()).unwrap())
            .unwrap(),
        request
    );
    for forbidden in ["bytes", "signedUrl", "localPath", "storageRoot"] {
        let mut invalid = request.clone();
        invalid[forbidden] = json!("private");
        assert!(serde_json::from_value::<FileRequest>(invalid).is_err());
    }
    for alias in [
        "../other", "a/../b", "C:/file", "NUL.txt", "COM9", "a.", "x:y", "a\\b", "a//b",
    ] {
        assert!(
            serde_json::from_value::<FilePath>(json!(alias)).is_err(),
            "{alias}"
        );
    }
    let entry: FileEntry =
        serde_json::from_value(json!({"path":"a.bin", "kind":"file", "byteLength":3})).unwrap();
    assert_eq!(entry.path.as_str(), "a.bin");
    assert!(
        serde_json::from_value::<FileEntry>(
            json!({"path":"a.bin", "kind":"file", "byteLength":3, "unknown":true})
        )
        .is_err()
    );
}
