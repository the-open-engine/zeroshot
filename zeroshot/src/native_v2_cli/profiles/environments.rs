//! Environment resources share the profile lock, so references cannot race deletion.
use fs2::FileExt;
use openengine_cluster_protocol::{
    EnvironmentId, ProfileRuntimePlan, ResolvedRunProfile, RunProfileSelector,
    RuntimeEnvironmentResource, RuntimePlan,
};
#[cfg(any(test, feature = "ui"))]
use openengine_cluster_protocol::{
    EnvironmentRevision, RuntimeEnvironmentDeleteRequest, RuntimeEnvironmentDeleteResult,
    RuntimeEnvironmentListResult, RuntimeEnvironmentSaveRequest, RuntimeEnvironmentSummary,
};
use super::{
    LocalRunProfileStore, NativeV2CliError, StoredProfiles, local_io, local_message, profile,
    require_local_scope,
};

impl LocalRunProfileStore {
    #[cfg(any(test, feature = "ui"))]
    pub(crate) fn environments(&self) -> Result<RuntimeEnvironmentListResult, NativeV2CliError> {
        let lock = self.lock()?;
        FileExt::lock_shared(&lock).map_err(local_io)?;
        let stored = self.read()?;
        Ok(RuntimeEnvironmentListResult {
            environments: stored
                .environments
                .values()
                .map(|value| RuntimeEnvironmentSummary {
                    id: value.id.clone(),
                    name: value.name.clone(),
                    revision: value.revision.clone(),
                })
                .collect(),
        })
    }

    pub(crate) fn environment(
        &self,
        id: &EnvironmentId,
    ) -> Result<RuntimeEnvironmentResource, NativeV2CliError> {
        let lock = self.lock()?;
        FileExt::lock_shared(&lock).map_err(local_io)?;
        self.read()?
            .environments
            .get(id)
            .cloned()
            .ok_or_else(|| NativeV2CliError::EnvironmentMissing(id.clone()))
    }

    #[cfg(any(test, feature = "ui"))]
    pub(crate) fn save_environment(
        &self,
        request: RuntimeEnvironmentSaveRequest,
        workspace: Option<&str>,
    ) -> Result<RuntimeEnvironmentResource, NativeV2CliError> {
        request
            .definition
            .validate()
            .map_err(|error| local_message(error.to_string()))?;
        let lock = self.lock()?;
        lock.lock_exclusive().map_err(local_io)?;
        self.check_environment_workspace(workspace)?;
        let mut stored = self.read()?;
        let id = checked_environment_id(&stored, &request)?;
        if stored
            .environments
            .values()
            .any(|value| value.id != id && value.name == request.name)
        {
            return Err(NativeV2CliError::EnvironmentConflict);
        }
        let resource = RuntimeEnvironmentResource {
            id: id.clone(),
            name: request.name,
            definition: request.definition,
            revision: EnvironmentRevision::new(uuid::Uuid::now_v7().to_string())
                .map_err(|error| local_message(error.to_string()))?,
        };
        stored.environments.insert(id, resource.clone());
        self.write(&stored)?;
        Ok(resource)
    }

    #[cfg(any(test, feature = "ui"))]
    pub(crate) fn delete_environment(
        &self,
        request: RuntimeEnvironmentDeleteRequest,
        workspace: Option<&str>,
    ) -> Result<RuntimeEnvironmentDeleteResult, NativeV2CliError> {
        let lock = self.lock()?;
        lock.lock_exclusive().map_err(local_io)?;
        self.check_environment_workspace(workspace)?;
        let mut stored = self.read()?;
        if stored
            .environments
            .get(&request.id)
            .map(|value| &value.revision)
            != Some(&request.expected_revision)
        {
            return Err(NativeV2CliError::EnvironmentConflict);
        }
        if stored.profiles.values().any(|profile| {
            profile
                .runtime
                .environment()
                .is_some_and(|reference| reference.id == request.id)
        }) {
            return Err(NativeV2CliError::EnvironmentInUse);
        }
        stored.environments.remove(&request.id);
        self.write(&stored)?;
        Ok(RuntimeEnvironmentDeleteResult { deleted: true })
    }

    #[cfg(any(test, feature = "ui"))]
    fn check_environment_workspace(&self, workspace: Option<&str>) -> Result<(), NativeV2CliError> {
        #[cfg(feature = "ui")]
        if let Some(workspace) = workspace {
            if super::read_workspace_id(&self.root.join("ui-workspace-id"))?.as_deref()
                != Some(workspace)
            {
                return Err(NativeV2CliError::EnvironmentWorkspaceChanged);
            }
        }
        #[cfg(not(feature = "ui"))]
        let _ = workspace;
        Ok(())
    }

    pub(crate) fn resolve_profile(
        &self,
        selector: RunProfileSelector,
    ) -> Result<ResolvedRunProfile, NativeV2CliError> {
        require_local_scope(selector.scope)?;
        let lock = self.lock()?;
        FileExt::lock_shared(&lock).map_err(local_io)?;
        let stored = self.read()?;
        let authored = profile(&stored, &selector.name)
            .ok_or_else(|| local_message(format!("profile {} was not found", selector.name)))?;
        let runtime = resolve_runtime(&stored, &authored.runtime)?;
        Ok(ResolvedRunProfile {
            id: authored.id,
            name: authored.name,
            scope: authored.scope,
            graph: authored.graph,
            runtime,
            is_default: authored.is_default,
        })
    }

    #[cfg(any(test, feature = "ui"))]
    pub(crate) fn resolve_runtime(
        &self,
        runtime: &ProfileRuntimePlan,
    ) -> Result<RuntimePlan, NativeV2CliError> {
        let lock = self.lock()?;
        FileExt::lock_shared(&lock).map_err(local_io)?;
        resolve_runtime(&self.read()?, runtime)
    }
}

#[cfg(any(test, feature = "ui"))]
fn checked_environment_id(
    stored: &StoredProfiles,
    request: &RuntimeEnvironmentSaveRequest,
) -> Result<EnvironmentId, NativeV2CliError> {
    match (&request.id, &request.expected_revision) {
        (None, None) => EnvironmentId::new(uuid::Uuid::now_v7().to_string())
            .map_err(|error| local_message(error.to_string())),
        (Some(id), Some(expected))
            if stored.environments.get(id).map(|value| &value.revision) == Some(expected) =>
        {
            Ok(id.clone())
        }
        _ => Err(NativeV2CliError::EnvironmentConflict),
    }
}

pub(super) fn resolve_runtime(
    stored: &StoredProfiles,
    runtime: &ProfileRuntimePlan,
) -> Result<RuntimePlan, NativeV2CliError> {
    let definition = runtime
        .environment()
        .map(|reference| {
            stored
                .environments
                .get(&reference.id)
                .map(|resource| resource.definition.clone())
                .ok_or_else(|| NativeV2CliError::EnvironmentMissing(reference.id.clone()))
        })
        .transpose()?;
    Ok(runtime.clone().map_environment(|_| definition))
}

#[cfg(test)]
mod tests;
