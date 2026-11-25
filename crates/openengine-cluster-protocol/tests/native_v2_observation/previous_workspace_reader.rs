//! Frozen pre-workspace (cluster/v2) reader; unknown fields must stay refused.
use openengine_cluster_protocol::*;
use serde::{Deserialize, Serialize};
use schemars::JsonSchema;
macro_rules! define_run_status_projection {
    (
        $(#[$attribute:meta])*
        $name:ident
        $(
            {
                run_id: #[$run_id_attribute:meta],
                title: #[$title_attribute:meta],
                source: #[$source_attribute:meta],
                size: #[$size_attribute:meta],
                at_cursor: #[$cursor_attribute:meta],
                status: #[$status_attribute:meta],
            }
        )?
    ) => {
        $(#[$attribute])*
        #[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        pub struct $name {
            $(#[$run_id_attribute])?
            pub run_id: RunId,
            $(#[$title_attribute])?
            pub title: RunTitle,
            $(#[$source_attribute])?
            #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ResolvedSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_sources: Vec<ResolvedSource>,
            $(#[$size_attribute])?
            pub size: RunSize,
            $(#[$cursor_attribute])?
            pub at_cursor: Cursor,
            $(#[$status_attribute])?
            pub status: RunStatus,
            /// Omitted for responding controllers and finished runs. Never durable run state.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub controller: Option<ControllerObservation>,
            #[serde(default, skip_serializing_if = "WorkspaceRecovery::is_empty")]
            pub workspace_recovery: WorkspaceRecovery,
            #[serde(default, flatten)]
            pub steering: openengine_cluster_protocol::RunSteeringStatus,
        }
    };
}

define_run_status_projection!(BaseRunStatusResult);

#[test]
fn populated_workspace_status_requires_new_observation_gates() {
    assert_eq!(PROTOCOL_VERSION, "openengine.cluster/v3");
    assert_eq!(TARGET_DISCOVERY_KIND, "zeroshot.native-v2-target/v4");
    assert_eq!(HOSTED_RUNS_KIND, "zeroshot.hosted-runs/v3");
    let current = RunStatusResult {
        run_id: RunId::new("workspace-status"),
        part_id: RunId::new("workspace-status"),
        current_part_id: RunId::new("workspace-status"),
        title: RunTitle::new("File workspace").unwrap(),
        source: None,
        context_sources: vec![],
        size: RunSize::new("host").unwrap(),
        at_cursor: Cursor::new("v2:1"),
        status: RunStatus::Admitted {},
        controller: None,
        workspace_recovery: Default::default(),
        steering: Default::default(),
        workspace: Some(RunWorkspace {
            placement: WorkspacePlacement::Managed,
            local_directory: None,
            seeds: vec![],
            copy_root: false,
        }),
    };
    let value = serde_json::to_value(&current).unwrap();
    assert!(serde_json::from_value::<RunStatusResult>(value.clone()).is_ok());
    let mut previous = value.clone();
    // Isolate the workspace incompatibility from the separately versioned part projection.
    for field in ["partId", "currentPartId"] {
        previous.as_object_mut().unwrap().remove(field);
    }
    assert!(
        serde_json::from_value::<BaseRunStatusResult>(previous.clone())
            .unwrap_err()
            .to_string()
            .contains("workspace")
    );
    previous.as_object_mut().unwrap().remove("workspace");
    assert!(serde_json::from_value::<BaseRunStatusResult>(previous).is_ok());
    let hosted = serde_json::to_value(HostedRunStatusResult::from(current)).unwrap();
    assert_eq!(hosted["workspace"], value["workspace"]);
}
