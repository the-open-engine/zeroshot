//! Shared host context translation used by all adapter factories.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use crate::harnesses::{HarnessBuildContext, HarnessError, HarnessHost};
use crate::native_v2_capsule::{HarnessProcessEnvironment, provider_process::LocalHarnessEnvironment};

pub(crate) fn local_parts(
    context: &HarnessBuildContext,
) -> (Option<PathBuf>, LocalHarnessEnvironment) {
    match &context.host {
        HarnessHost::Local { home, environment } => (home.clone(), environment.clone()),
        HarnessHost::Contained { .. } => (None, LocalHarnessEnvironment::default()),
    }
}

pub(crate) fn pool(context: &HarnessBuildContext) -> crate::execution::process::HostedProcessPool {
    match context.host {
        HarnessHost::Contained { process_pool } => process_pool,
        HarnessHost::Local { .. } => crate::execution::process::HostedProcessPool::hosted_default(),
    }
}

pub(crate) fn process_environment(
    context: &HarnessBuildContext,
) -> Result<HarnessProcessEnvironment, HarnessError> {
    let mut values = BTreeMap::from([("PATH".to_owned(), context.search_path.clone())]);
    if let HarnessHost::Local { environment, .. } = &context.host {
        for name in ["LANG", "LC_ALL", "TERM", "TMPDIR"] {
            if let Some(value) = environment.get(name) {
                values.insert(name.to_owned(), value.clone());
            }
        }
    }
    values.extend(context.base_environment.clone());
    let environment = HarnessProcessEnvironment::new(values)
        .map_err(|error| HarnessError::Configuration(error.to_string()))?;
    if context.host.is_local() {
        Ok(environment)
    } else {
        environment
            .for_capsule(&context.runtime_home, &context.search_path)
            .map_err(|error| HarnessError::Configuration(error.to_string()))
    }
}

pub(crate) fn normalize_paths(
    environment: &mut LocalHarnessEnvironment,
    directory: &Path,
    names: &[&str],
) -> Result<(), HarnessError> {
    for name in names {
        let Some(value) = environment.get_mut(name) else {
            continue;
        };
        let path = PathBuf::from(&*value);
        if !value.is_empty() && path.is_relative() {
            *value = directory
                .join(path)
                .into_os_string()
                .into_string()
                .map_err(|_| {
                    HarnessError::Configuration("native home path is not valid UTF-8".to_owned())
                })?;
        }
    }
    Ok(())
}

/// Security-sensitive native context translation is identical for every typed provider factory.
pub(crate) struct NativeFactoryInputs<P> {
    pub(crate) provider: P,
    pub(crate) home: Option<PathBuf>,
    pub(crate) environment: LocalHarnessEnvironment,
    pub(crate) local: bool,
    pub(crate) process_pool: crate::execution::process::HostedProcessPool,
}

impl<P: std::str::FromStr> NativeFactoryInputs<P> {
    pub(crate) fn new(context: &HarnessBuildContext) -> Result<Self, HarnessError> {
        let provider = context
            .lane
            .provider_name()
            .parse()
            .map_err(|_| HarnessError::UnsupportedProvider(context.lane.clone()))?;
        let (home, environment) = local_parts(context);
        Ok(Self {
            provider,
            home,
            environment,
            local: context.host.is_local(),
            process_pool: pool(context),
        })
    }
}

pub(crate) fn parsed_provider_access<P: std::str::FromStr>(
    provider: &str,
    access: fn(P) -> crate::harnesses::ProviderAccessContract,
) -> Result<crate::harnesses::ProviderAccessContract, HarnessError> {
    let provider = provider
        .parse()
        .map_err(|_| HarnessError::Configuration("invalid provider".to_owned()))?;
    Ok(access(provider))
}

pub(crate) fn executable_text(path: &Path) -> Result<String, HarnessError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| HarnessError::Configuration("executable path is invalid".to_owned()))
}
