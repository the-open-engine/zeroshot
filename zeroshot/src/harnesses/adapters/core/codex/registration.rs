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
            id: "codex",
            label: "Codex",
            providers: &["openai", "openrouter", "gateway", "bedrock"],
            executable: "codex",
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
    normalize_paths(environment, directory, &["CODEX_HOME"])
}
fn build(
    context: HarnessBuildContext,
    memory: Option<Arc<ActiveMemoryHooks>>,
) -> Result<HarnessAdapter, HarnessError> {
    let native = NativeFactoryInputs::<crate::native_v2_contract::CodexProvider>::new(&context)?;
    let local_user = native.home.map(|home| super::NativeV2CodexUser {
        codex_home: native
            .environment
            .get("CODEX_HOME")
            .filter(|value| !value.is_empty())
            .map_or_else(|| home.join(".codex"), std::path::PathBuf::from),
        home,
    });
    let config = super::NativeV2CodexConfig {
        provider: native.provider,
        executable: context.executable,
        workspace: context.workspace,
        runtime_home: context.runtime_home,
        local_user,
        native_environment: native.environment,
        base_environment: context.base_environment,
        search_path: context.search_path,
        process_pool: native.process_pool,
    };
    let adapter = if native.local {
        super::NativeV2CodexAdapter::new_local(config)
    } else {
        super::NativeV2CodexAdapter::new(config)
    };
    Ok(HarnessAdapter::new(adapter.with_active_memory(memory)))
}
