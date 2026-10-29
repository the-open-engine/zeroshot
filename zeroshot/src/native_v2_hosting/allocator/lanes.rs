//! Contained placement selects only the host's enabled core registrations.
use crate::execution::process::HostedProcessPool;
use crate::harnesses::{HarnessBuildContext, HarnessHost, HarnessRegistry};
use crate::native_v2_candidate::NativeV2HarnessConfig;
use crate::native_v2_capsule::CapsuleFilesystem;
use crate::native_v2_contract::AdmittedRun;
use crate::native_v2_platform::CapsuleAllocationUnavailable;
use crate::native_v2_hosting::environment;
use super::ProductionCapsuleAllocator;

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
        let registry = HarnessRegistry::core();
        admitted
            .runtime
            .lanes()
            .into_iter()
            .map(|lane| {
                let launch = crate::harnesses::adapters::core::contained_launch(
                    &self.config,
                    lane.harness_name(),
                )
                .ok_or(CapsuleAllocationUnavailable::Runtime)?;
                let mut base_environment = launch.environment;
                base_environment.insert("ZEROSHOT_TOOLS".to_owned(), tools_directory.clone());
                registry
                    .configure(HarnessBuildContext {
                        lane,
                        workspace: filesystem.workspace.clone(),
                        runtime_home: filesystem.runtime_home.clone(),
                        executable: launch.executable,
                        prefix_arguments: launch.arguments,
                        search_path: search_path.clone(),
                        base_environment,
                        host: HarnessHost::Contained { process_pool },
                    })
                    .map_err(|_| CapsuleAllocationUnavailable::Runtime)
            })
            .collect()
    }
}
