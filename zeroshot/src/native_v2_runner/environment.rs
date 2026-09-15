use super::*;

/// Runtime-only environment values. Debug output exposes names, never values.
#[derive(Clone)]
pub struct ResolvedEnvironment {
    values: Arc<BTreeMap<EnvironmentVariableName, String>>,
    refresh: Option<Arc<dyn RuntimeEnvironmentRefresh>>,
}

#[async_trait]
pub(crate) trait RuntimeEnvironmentRefresh: Send + Sync {
    async fn refresh(&self) -> Result<ResolvedEnvironment, EnvironmentRefreshError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum EnvironmentRefreshError {
    #[error("runtime environment refresh is temporarily unavailable")]
    Unavailable,
    #[error("runtime environment refresh was refused")]
    Refused,
    #[error("runtime environment refresh returned an invalid response")]
    InvalidResponse,
}

impl ResolvedEnvironment {
    pub fn exact(
        binding: &NodeRuntimeBinding,
        values: BTreeMap<EnvironmentVariableName, String>,
    ) -> Result<Self, EnvironmentResolutionError> {
        let declared = binding.declared_connections();
        if let Some(name) = declared
            .environment_names()
            .find(|name| !values.contains_key(*name))
        {
            return Err(EnvironmentResolutionError::Missing(name.clone()));
        }
        if let Some(name) = values.keys().find(|name| {
            !declared
                .environment_names()
                .any(|declared| declared == *name)
        }) {
            return Err(EnvironmentResolutionError::Undeclared(name.clone()));
        }
        Ok(Self {
            values: Arc::new(values),
            refresh: None,
        })
    }

    #[must_use]
    pub fn get(&self, name: &EnvironmentVariableName) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub(crate) fn can_refresh(&self) -> bool {
        self.refresh.is_some()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&EnvironmentVariableName, &str)> {
        self.values
            .iter()
            .map(|(name, value)| (name, value.as_str()))
    }
}

pub(super) fn with_refresh(
    mut environment: ResolvedEnvironment,
    refresh: Arc<dyn RuntimeEnvironmentRefresh>,
) -> ResolvedEnvironment {
    environment.refresh = Some(refresh);
    environment
}

pub(super) async fn refreshed(
    environment: &ResolvedEnvironment,
) -> Result<ResolvedEnvironment, EnvironmentRefreshError> {
    match &environment.refresh {
        Some(refresh) => refresh.refresh().await,
        None => Ok(environment.clone()),
    }
}

impl fmt::Debug for ResolvedEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResolvedEnvironment")
            .field("names", &self.values.keys().collect::<Vec<_>>())
            .field("values", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EnvironmentResolutionError {
    #[error("declared environment variable {0} was not resolved")]
    Missing(EnvironmentVariableName),
    #[error("environment variable {0} was not declared by the node")]
    Undeclared(EnvironmentVariableName),
}
