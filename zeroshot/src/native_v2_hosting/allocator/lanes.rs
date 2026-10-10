//! A hosted capsule builds one harness configuration per distinct effective lane.
use std::collections::BTreeMap;

use crate::execution::process::HostedProcessPool;
use crate::native_v2_candidate::NativeV2HarnessConfig;
use crate::native_v2_capsule::CapsuleFilesystem;
use crate::native_v2_claude::{ClaudeAdapterConfig, ClaudeProcessEnvironment};
use crate::native_v2_cloud::CapsuleAllocationUnavailable;
use crate::native_v2_codex::NativeV2CodexConfig;
use crate::native_v2_contract::{AdmittedRun, RuntimeLane};
use crate::native_v2_hosting::environment;
use super::ProductionCapsuleAllocator;

#[derive(Clone, Copy)]
struct HostedLaneInputs<'a> {
    filesystem: &'a CapsuleFilesystem,
    search_path: &'a str,
    tools_directory: &'a str,
    base_environment: &'a BTreeMap<String, String>,
    process_pool: HostedProcessPool,
}

impl ProductionCapsuleAllocator {
    pub(super) fn lanes(
        &self,
        admitted: &AdmittedRun,
        filesystem: &CapsuleFilesystem,
        process_pool: HostedProcessPool,
    ) -> Result<Vec<NativeV2HarnessConfig>, CapsuleAllocationUnavailable> {
        let run_root = filesystem
            .workspace
            .parent()
            .ok_or(CapsuleAllocationUnavailable::Runtime)?;
        let search_path = environment::search_path(run_root, &self.config.executable_search_path);
        let tools_directory = environment::tools_directory(run_root)
            .to_string_lossy()
            .into_owned();
        let base_environment =
            BTreeMap::from([("ZEROSHOT_TOOLS".to_owned(), tools_directory.clone())]);
        let inputs = HostedLaneInputs {
            filesystem,
            search_path: &search_path,
            tools_directory: &tools_directory,
            base_environment: &base_environment,
            process_pool,
        };
        admitted
            .runtime
            .lanes()
            .into_iter()
            .map(|lane| self.lane_harness(lane, inputs))
            .collect()
    }

    fn lane_harness(
        &self,
        lane: RuntimeLane,
        inputs: HostedLaneInputs<'_>,
    ) -> Result<NativeV2HarnessConfig, CapsuleAllocationUnavailable> {
        let HostedLaneInputs {
            filesystem,
            search_path,
            tools_directory,
            base_environment,
            process_pool,
        } = inputs;
        match lane {
            RuntimeLane::Copilot { .. } => Ok(NativeV2HarnessConfig::Copilot(
                crate::native_v2_copilot::CopilotConfig {
                    executable: self.config.copilot_executable.clone(),
                    workspace: filesystem.workspace.clone(),
                    runtime_home: filesystem.runtime_home.clone(),
                    local_user: None,
                    base_environment: base_environment.clone(),
                    local_command_environment: std::collections::BTreeMap::new(),
                    search_path: search_path.to_owned(),
                    process_pool,
                },
            )),
            RuntimeLane::Codex { provider } => {
                Ok(NativeV2HarnessConfig::Codex(NativeV2CodexConfig {
                    provider,
                    executable: self.config.codex_executable.clone(),
                    workspace: filesystem.workspace.clone(),
                    runtime_home: filesystem.runtime_home.clone(),
                    local_user: None,
                    native_environment: Default::default(),
                    base_environment: base_environment.clone(),
                    search_path: search_path.to_owned(),
                    process_pool,
                }))
            }
            RuntimeLane::Claude { provider } => {
                let base_environment = self
                    .config
                    .claude_process_environment
                    .for_capsule(&filesystem.runtime_home, search_path)
                    .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
                let mut values = base_environment.clone_values();
                values.insert("ZEROSHOT_TOOLS".to_owned(), tools_directory.to_owned());
                let base_environment = ClaudeProcessEnvironment::new(values)
                    .map_err(|_| CapsuleAllocationUnavailable::Runtime)?;
                Ok(NativeV2HarnessConfig::Claude(ClaudeAdapterConfig {
                    provider,
                    executable: self.config.claude_executable.clone(),
                    prefix_arguments: self.config.claude_prefix_arguments.clone(),
                    workspace: filesystem.workspace.clone(),
                    runtime_home: filesystem.runtime_home.clone(),
                    local_user_home: None,
                    native_environment: Default::default(),
                    base_environment,
                    process_pool,
                }))
            }
        }
    }
}
