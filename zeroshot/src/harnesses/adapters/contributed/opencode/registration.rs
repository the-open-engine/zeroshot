use std::path::Path;
use std::sync::Arc;

use crate::harnesses::{
    HarnessAdapter, HarnessBuildContext, HarnessDescriptor, HarnessError, HarnessRegistration,
    HarnessSource, ProviderAccessContract,
};
use crate::harnesses::runtime::{local_parts, normalize_paths, pool, process_environment};
use crate::native_v2_candidate::ActiveMemoryHooks;
use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;

use super::{OpenCodeAdapter, OpenCodeConfig};

pub(crate) fn registration() -> HarnessRegistration {
    HarnessRegistration {
        descriptor: HarnessDescriptor {
            id: "opencode",
            label: "OpenCode",
            providers: &[
                "openai",
                "anthropic",
                "openrouter",
                "bedrock",
                "gateway",
                "github",
            ],
            executable: "opencode",
            source: HarnessSource::Contributed,
        },
        build,
        access,
        normalize_local,
    }
}

fn build(
    context: HarnessBuildContext,
    memory: Option<Arc<ActiveMemoryHooks>>,
) -> Result<HarnessAdapter, HarnessError> {
    let (home, environment) = local_parts(&context);
    let config = OpenCodeConfig {
        provider: context.lane.provider_name().to_owned(),
        executable: context
            .executable
            .to_str()
            .ok_or_else(|| {
                HarnessError::Configuration("OpenCode executable path is invalid UTF-8".to_owned())
            })?
            .to_owned(),
        prefix_arguments: context.prefix_arguments.clone(),
        workspace: context.workspace.clone(),
        runtime_home: context.runtime_home.clone(),
        local_user_home: home,
        native_environment: environment,
        base_environment: process_environment(&context)?,
        process_pool: pool(&context),
    };
    let adapter = if context.host.is_local() {
        OpenCodeAdapter::new_local(config)
    } else {
        OpenCodeAdapter::new(config)
    }
    .map_err(|error| HarnessError::Configuration(error.to_string()))?;
    Ok(HarnessAdapter::new(adapter.with_active_memory(memory)))
}

fn access(provider: &str) -> Result<ProviderAccessContract, HarnessError> {
    match provider {
        "openai" => Ok(ProviderAccessContract::new(
            true,
            "openai",
            &["OPENAI_API_KEY"],
            &[&["OPENAI_API_KEY"]],
        )),
        "anthropic" => Ok(ProviderAccessContract::new(
            true,
            "anthropic",
            &["ANTHROPIC_API_KEY"],
            &[&["ANTHROPIC_API_KEY"]],
        )),
        "openrouter" => Ok(ProviderAccessContract::new(
            true,
            "openrouter",
            &["OPENROUTER_API_KEY"],
            &[&["OPENROUTER_API_KEY"]],
        )),
        "bedrock" => Ok(ProviderAccessContract::new(
            true,
            "bedrock",
            &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
            &[&["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"]],
        )),
        "github" => Ok(ProviderAccessContract::new(
            true,
            "github",
            &["COPILOT_GITHUB_TOKEN"],
            &[&["COPILOT_GITHUB_TOKEN"]],
        )),
        "gateway" => Ok(ProviderAccessContract::new(
            false,
            "gateway",
            &["GATEWAY_BASE_URL", "GATEWAY_API_KEY", "GATEWAY_API"],
            &[&["GATEWAY_BASE_URL", "GATEWAY_API_KEY", "GATEWAY_API"]],
        )),
        _ => Err(HarnessError::Configuration(
            "OpenCode provider is unsupported".to_owned(),
        )),
    }
}

fn normalize_local(
    environment: &mut LocalHarnessEnvironment,
    directory: &Path,
) -> Result<(), HarnessError> {
    normalize_paths(
        environment,
        directory,
        &[
            "AWS_CONFIG_FILE",
            "AWS_SHARED_CREDENTIALS_FILE",
            "AWS_WEB_IDENTITY_TOKEN_FILE",
            "AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
            "ProgramData",
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_DIR",
            "OPENCODE_MODELS_PATH",
            "OPENCODE_GIT_BASH_PATH",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_STATE_HOME",
        ],
    )
}
