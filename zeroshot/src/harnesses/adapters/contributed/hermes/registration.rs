use std::path::Path;
use std::sync::Arc;

use crate::harnesses::{
    HarnessAdapter, HarnessBuildContext, HarnessDescriptor, HarnessError, HarnessRegistration,
    HarnessSource, ProviderAccessContract,
};
use crate::harnesses::runtime::{local_parts, normalize_paths, pool, process_environment};
use crate::native_v2_candidate::ActiveMemoryHooks;
use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;

use super::{HermesAdapter, HermesConfig};

pub(crate) fn registration() -> HarnessRegistration {
    HarnessRegistration {
        descriptor: HarnessDescriptor {
            id: "hermes",
            label: "Hermes",
            providers: &[
                "openai",
                "anthropic",
                "openrouter",
                "gateway",
                "github",
                "bedrock",
            ],
            executable: "hermes",
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
    let config = HermesConfig {
        provider: context.lane.provider_name().to_owned(),
        executable: crate::harnesses::runtime::executable_text(&context.executable)?,
        prefix_arguments: context.prefix_arguments.clone(),
        workspace: context.workspace.clone(),
        runtime_home: context.runtime_home.clone(),
        local_user_home: home,
        native_environment: environment,
        base_environment: process_environment(&context)?,
        process_pool: pool(&context),
    };
    let adapter = if context.host.is_local() {
        HermesAdapter::new_local(config)
    } else {
        HermesAdapter::new(config)
    };
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
            &[
                &["ANTHROPIC_API_KEY"],
                &["ANTHROPIC_TOKEN"],
                &["CLAUDE_CODE_OAUTH_TOKEN"],
            ],
        )),
        "openrouter" => Ok(ProviderAccessContract::new(
            true,
            "openrouter",
            &["OPENROUTER_API_KEY"],
            &[&["OPENROUTER_API_KEY"]],
        )),
        "github" => Ok(ProviderAccessContract::new(
            true,
            "github",
            &["COPILOT_GITHUB_TOKEN"],
            &[&["COPILOT_GITHUB_TOKEN"], &["GH_TOKEN"], &["GITHUB_TOKEN"]],
        )),
        "bedrock" => Ok(ProviderAccessContract::new(
            true,
            "bedrock",
            &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
            &[
                &["AWS_BEARER_TOKEN_BEDROCK", "AWS_REGION"],
                &["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_REGION"],
                &["AWS_PROFILE", "AWS_REGION"],
            ],
        )),
        "gateway" => Ok(ProviderAccessContract::new(
            false,
            "gateway",
            &["GATEWAY_BASE_URL", "GATEWAY_API_KEY", "GATEWAY_API"],
            &[&["GATEWAY_BASE_URL", "GATEWAY_API_KEY", "GATEWAY_API"]],
        )),
        _ => Err(HarnessError::Configuration(
            "Hermes provider is unsupported".to_owned(),
        )),
    }
}

fn normalize_local(
    environment: &mut LocalHarnessEnvironment,
    directory: &Path,
) -> Result<(), HarnessError> {
    normalize_paths(environment, directory, super::command::NATIVE_PATHS)?;
    if let Some(value) = environment.get_mut("PYTHONPATH") {
        let paths = std::env::split_paths(value.as_str()).map(|path| {
            if path.is_absolute() {
                path
            } else {
                directory.join(path)
            }
        });
        *value = std::env::join_paths(paths)
            .map_err(|_| HarnessError::Configuration("Hermes PYTHONPATH is invalid".to_owned()))?
            .into_string()
            .map_err(|_| {
                HarnessError::Configuration("Hermes PYTHONPATH is not UTF-8".to_owned())
            })?;
    }
    Ok(())
}
