use std::path::Path;
use std::sync::Arc;

use crate::active_memory::hooks::ActiveMemoryHooks;
use crate::harnesses::{
    HarnessAdapter, HarnessBuildContext, HarnessDescriptor, HarnessError, HarnessRegistration,
    HarnessSource,
};
use crate::harnesses::runtime::{
    NativeFactoryInputs, parsed_provider_access, executable_text, process_environment,
    normalize_paths,
};
use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;

pub(crate) fn registration() -> HarnessRegistration {
    HarnessRegistration {
        descriptor: HarnessDescriptor {
            id: "pi",
            label: "Pi",
            providers: &["anthropic", "openai", "openrouter", "gateway", "bedrock"],
            executable: "pi",
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
    let home = environment
        .user_home()
        .map(std::path::PathBuf::from)
        .or_else(crate::execution::platform::user_home);
    if let Some(value) = environment.get_mut("PI_CODING_AGENT_DIR") {
        if value.starts_with("file://") {
            return Ok(());
        }
        let path = crate::native_v2_local::pi_agent_directory_path(value, home.as_deref())
            .map_err(|_| HarnessError::Configuration("Pi agent directory is invalid".to_owned()))?;
        *value = path.to_string_lossy().into_owned();
    }
    normalize_paths(environment, directory, &["PI_CODING_AGENT_DIR"])
}
fn build(
    context: HarnessBuildContext,
    memory: Option<Arc<ActiveMemoryHooks>>,
) -> Result<HarnessAdapter, HarnessError> {
    let native = NativeFactoryInputs::<crate::native_v2_contract::PiProvider>::new(&context)?;
    let base_environment = process_environment(&context)?;
    let executable = executable_text(&context.executable)?;
    let config = super::PiConfig {
        provider: native.provider,
        executable,
        prefix_arguments: context.prefix_arguments,
        workspace: context.workspace,
        runtime_home: context.runtime_home,
        local_user_home: native.home,
        native_environment: native.environment,
        base_environment,
        process_pool: native.process_pool,
    };
    let adapter = if native.local {
        super::PiAdapter::new_local(config)
    } else {
        super::PiAdapter::new(config)
    }
    .map_err(|error| HarnessError::Configuration(error.to_string()))?;
    Ok(HarnessAdapter::new(adapter.with_active_memory(memory)))
}
