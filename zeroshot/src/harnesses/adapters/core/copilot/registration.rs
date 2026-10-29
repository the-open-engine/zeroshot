use std::path::Path;
use std::sync::Arc;

use crate::active_memory::hooks::ActiveMemoryHooks;
use crate::harnesses::{
    HarnessAdapter, HarnessBuildContext, HarnessDescriptor, HarnessError, HarnessRegistration,
    HarnessSource,
};
use crate::harnesses::runtime::{NativeFactoryInputs, parsed_provider_access, normalize_paths};
use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;

pub(crate) fn registration() -> HarnessRegistration {
    HarnessRegistration {
        descriptor: HarnessDescriptor {
            id: "copilot",
            label: "GitHub Copilot",
            providers: &[
                "github",
                "openai",
                "anthropic",
                "openrouter",
                "gateway",
                "bedrock",
            ],
            executable: "copilot",
            source: HarnessSource::Core,
        },
        build,
        access: |provider| parsed_provider_access(provider, super::access::provider_access),
        normalize_local,
    }
}
fn normalize_local(
    environment: &mut LocalHarnessEnvironment,
    directory: &Path,
) -> Result<(), HarnessError> {
    normalize_paths(
        environment,
        directory,
        &["COPILOT_HOME", "COPILOT_PROVIDERS_CONFIG"],
    )
}
fn build(
    context: HarnessBuildContext,
    memory: Option<Arc<ActiveMemoryHooks>>,
) -> Result<HarnessAdapter, HarnessError> {
    let native = NativeFactoryInputs::<crate::native_v2_contract::CopilotProvider>::new(&context)?;
    let local_user = native.home.map(|home| super::CopilotLocalUser {
        copilot_home: native
            .environment
            .get("COPILOT_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(".copilot"), std::path::PathBuf::from),
        home,
    });
    let mut base_environment = native
        .environment
        .selected(super::command::LOCAL_ENVIRONMENT);
    base_environment.extend(context.base_environment);
    let config = super::CopilotConfig {
        executable: context.executable,
        workspace: context.workspace,
        runtime_home: context.runtime_home,
        local_user,
        base_environment,
        local_command_environment: native.environment.into_values(),
        search_path: context.search_path,
        process_pool: native.process_pool,
    };
    let adapter = if native.local {
        super::CopilotAdapter::new_local(config)
    } else {
        super::CopilotAdapter::new(config)
    };
    Ok(HarnessAdapter::new(
        adapter
            .with_provider(native.provider)
            .with_active_memory(memory),
    ))
}
