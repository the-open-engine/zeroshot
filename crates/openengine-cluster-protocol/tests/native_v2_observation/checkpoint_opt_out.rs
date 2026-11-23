use openengine_cluster_protocol::{
    TargetAuthentication, TargetDiscoveryDocument, TargetDiscoveryExtensions,
    WORKSPACE_CHECKPOINT_OPT_OUT_KIND,
};
use serde::Deserialize;
use serde_json::json;

#[test]
fn checkpoint_opt_out_is_an_independent_strict_versioned_marker() {
    for authentication in [
        TargetAuthentication::None,
        TargetAuthentication::HostedOauth,
    ] {
        let default = TargetDiscoveryDocument::direct(authentication);
        assert!(default.extensions.workspace_checkpoint_opt_out.is_none());
        let document = default.with_workspace_checkpoint_opt_out();
        let wire = serde_json::to_value(&document).unwrap();
        assert_eq!(
            wire["extensions"]["workspace_checkpoint_opt_out"],
            json!({
                "kind": WORKSPACE_CHECKPOINT_OPT_OUT_KIND
            })
        );
        assert!(!document.extensions.is_empty());
        assert!(document.extensions.workspace_checkpoints.is_none());
        let decoded: TargetDiscoveryDocument = serde_json::from_value(wire).unwrap();
        assert_eq!(decoded, document);
    }
    for marker in [
        json!({}),
        json!({"kind": WORKSPACE_CHECKPOINT_OPT_OUT_KIND, "enabled": true}),
    ] {
        assert!(
            serde_json::from_value::<TargetDiscoveryExtensions>(json!({
                "workspace_checkpoint_opt_out": marker
            }))
            .is_err()
        );
    }
    // Older discovery readers ignore the new outer extension; frozen marker objects stay closed.
    #[derive(Deserialize)]
    struct LegacyExtensions {
        #[serde(default)]
        workspace_checkpoints:
            Option<openengine_cluster_protocol::TargetWorkspaceCheckpointsDiscovery>,
    }
    let extensions =
        json!({"workspace_checkpoint_opt_out": {"kind": WORKSPACE_CHECKPOINT_OPT_OUT_KIND}});
    let legacy: LegacyExtensions = serde_json::from_value(extensions).unwrap();
    assert!(legacy.workspace_checkpoints.is_none());
}
