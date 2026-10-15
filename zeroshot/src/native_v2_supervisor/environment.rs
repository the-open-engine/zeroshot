use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::{
    ConnectionKey, EnvironmentVariableName, RunConnectionRequirements, RunConnectionValues,
    RuntimePlan, RuntimeEnvironment, StaticConnectionValues, run_connection_requirements,
    GitTransportCredentials, ResolvedSource, RunSubmission,
};
use thiserror::Error;

use crate::native_v2_contract::{AdmittedRun, NodeRuntimeBinding};
use crate::native_v2_runner::{
    EnvironmentRefreshError, ResolvedEnvironment, RuntimeEnvironmentRefresh,
    with_environment_refresh,
};

const MAX_RUN_ENVIRONMENT_BYTES: usize = 256 * 1024;

/// Resolves one node's exact dynamic connection shape at the moment that node starts.
#[async_trait]
pub(crate) trait RunConnectionResolver: Send + Sync {
    async fn resolve(
        &self,
        requirements: RunConnectionRequirements,
    ) -> Result<RunConnectionValues, ConnectionResolutionError>;
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum ConnectionResolutionError {
    #[error("dynamic connection resolution is temporarily unavailable")]
    Unavailable,
    #[error("dynamic connection resolution was refused")]
    Refused,
    #[error("dynamic connection resolution returned an invalid response")]
    InvalidResponse,
}

#[derive(Clone)]
pub(crate) struct DynamicConnectionPlan {
    pub resolver: Arc<dyn RunConnectionResolver>,
    pub keys: BTreeSet<ConnectionKey>,
    pub source_connection: Option<ConnectionKey>,
}

/// Bounded static values plus run-scoped authority for exactly the dynamic keys in one runtime.
///
/// Static values never enter admission, the ledger, observations, or debug output. Dynamic values
/// are fetched only for the connection keys and fields declared by the node that is starting.
/// Source-only fields stay private unless a node or preparation hook also explicitly declares them.
#[derive(Clone)]
pub struct RunEnvironment {
    values: Arc<RunConnectionValues>,
    variables: Arc<BTreeMap<EnvironmentVariableName, String>>,
    dynamic_keys: Arc<BTreeSet<ConnectionKey>>,
    source_connection: Option<ConnectionKey>,
    source_authorization: Arc<SourceAuthorization>,
    requirements: Arc<ConnectionRequirements>,
    resolver: Option<Arc<dyn RunConnectionResolver>>,
}

impl RunEnvironment {
    pub fn exact(
        runtime: &RuntimePlan,
        definition: Option<&RuntimeEnvironment>,
        values: RunConnectionValues,
    ) -> Result<Self, RunEnvironmentError> {
        Self::from_admission(EnvironmentAdmission::new(runtime, definition), values, None)
    }

    /// Bind source authority without automatically adding source fields to agent bindings.
    pub fn exact_for_submission(
        submission: &RunSubmission,
        values: RunConnectionValues,
    ) -> Result<Self, RunEnvironmentError> {
        Self::from_admission(
            EnvironmentAdmission::for_submission(submission),
            values,
            None,
        )
    }

    pub fn exact_for_admitted(
        admitted: &AdmittedRun,
        values: RunConnectionValues,
    ) -> Result<Self, RunEnvironmentError> {
        Self::from_admission(EnvironmentAdmission::for_admitted(admitted), values, None)
    }

    #[cfg(test)]
    pub(crate) fn with_resolver(
        runtime: &RuntimePlan,
        definition: Option<&RuntimeEnvironment>,
        values: RunConnectionValues,
        dynamic: DynamicConnectionPlan,
    ) -> Result<Self, RunEnvironmentError> {
        Self::from_admission(
            EnvironmentAdmission::new(runtime, definition),
            values,
            Some(dynamic),
        )
    }

    pub(crate) fn with_resolver_for_submission(
        submission: &RunSubmission,
        values: RunConnectionValues,
        dynamic: DynamicConnectionPlan,
    ) -> Result<Self, RunEnvironmentError> {
        Self::from_admission(
            EnvironmentAdmission::for_submission(submission),
            values,
            Some(dynamic),
        )
    }

    pub(crate) fn with_resolver_for_admitted(
        admitted: &AdmittedRun,
        values: RunConnectionValues,
        dynamic: DynamicConnectionPlan,
    ) -> Result<Self, RunEnvironmentError> {
        Self::from_admission(
            EnvironmentAdmission::for_admitted(admitted),
            values,
            Some(dynamic),
        )
    }

    fn from_admission(
        admission: EnvironmentAdmission,
        values: RunConnectionValues,
        dynamic: Option<DynamicConnectionPlan>,
    ) -> Result<Self, RunEnvironmentError> {
        let (keys, source, resolver) = match dynamic {
            Some(dynamic) => {
                validate_dynamic_plan(&dynamic)?;
                (
                    dynamic.keys,
                    dynamic.source_connection,
                    Some(dynamic.resolver),
                )
            }
            None => (BTreeSet::new(), None, None),
        };
        admission.source.validate_resolver(&keys, source.as_ref())?;
        validate_bootstrap(&admission.requirements, &values, &keys)?;
        let source_connection = admission.source.connection(source);
        Ok(Self {
            values: Arc::new(values),
            variables: admission.variables,
            dynamic_keys: Arc::new(keys),
            source_connection,
            source_authorization: Arc::new(admission.source),
            requirements: Arc::new(admission.requirements),
            resolver,
        })
    }

    /// Selects one run's exact keyed snapshots from a trusted host environment inventory.
    pub fn from_available(
        runtime: &RuntimePlan,
        definition: Option<&RuntimeEnvironment>,
        available: &BTreeMap<EnvironmentVariableName, String>,
    ) -> Result<Self, RunEnvironmentError> {
        let mut values = RunConnectionValues::new();
        for (key, fields) in run_connection_requirements(runtime, definition) {
            let selected = select_environment_values(key.clone(), fields.iter(), available)?;
            values.insert(
                key,
                StaticConnectionValues::new(selected)
                    .map_err(|_| RunEnvironmentError::InvalidPlan)?,
            );
        }
        Self::exact(runtime, definition, values)
    }

    /// Revalidates the static/dynamic partition against an immutable admitted runtime plan.
    pub fn for_runtime(
        &self,
        runtime: &RuntimePlan,
        definition: Option<&RuntimeEnvironment>,
    ) -> Result<Self, RunEnvironmentError> {
        let admission = EnvironmentAdmission::new(runtime, definition)
            .with_authorization(self.source_authorization.as_ref().clone());
        self.rebind(admission)
    }

    /// Verify the original source authorization against the immutable admitted run.
    pub fn for_admitted(&self, admitted: &AdmittedRun) -> Result<Self, RunEnvironmentError> {
        let admission = EnvironmentAdmission::for_admitted(admitted);
        if admission.source.requirements() != self.source_authorization.requirements() {
            return Err(RunEnvironmentError::InvalidPlan);
        }
        self.rebind(admission)
    }

    fn rebind(&self, admission: EnvironmentAdmission) -> Result<Self, RunEnvironmentError> {
        let dynamic = self
            .resolver
            .as_ref()
            .map(|resolver| DynamicConnectionPlan {
                resolver: resolver.clone(),
                keys: self.dynamic_keys.as_ref().clone(),
                source_connection: self
                    .source_connection
                    .clone()
                    .filter(|key| self.dynamic_keys.contains(key)),
            });
        Self::from_admission(admission, self.values.as_ref().clone(), dynamic)
    }

    /// Refresh only the declared generic source authority; native/public sources return None.
    pub(crate) async fn resolve_git_source(
        &self,
    ) -> Result<Option<GitTransportCredentials>, RunEnvironmentError> {
        let SourceAuthorization::Git { fields, .. } = self.source_authorization.as_ref() else {
            return Ok(None);
        };
        let values = self
            .resolve_source(fields.clone())
            .await?
            .ok_or(RunEnvironmentError::InvalidPlan)?;
        GitTransportCredentials::from_connection_values(&values)
            .map_err(|_| RunEnvironmentError::InvalidPlan)
    }

    pub(crate) fn has_git_source_connection(&self) -> bool {
        matches!(
            self.source_authorization.as_ref(),
            SourceAuthorization::Git { .. }
        )
    }

    /// Resolve only the explicitly selected hook connection fields; never inherit node credentials.
    pub(crate) async fn preparation_values(
        &self,
        environment: &RuntimeEnvironment,
    ) -> Result<BTreeMap<String, String>, RunEnvironmentError> {
        let requirements = declared_requirements(&environment.connections);
        let resolved = self.resolve_requirements(requirements).await?;
        let mut values: BTreeMap<String, String> = environment
            .variables
            .iter()
            .map(|(key, value)| (key.as_str().to_owned(), value.clone()))
            .collect();
        for connection in resolved.values() {
            values.extend(
                connection
                    .as_map()
                    .iter()
                    .map(|(key, value)| (key.as_str().to_owned(), value.clone())),
            );
        }
        Ok(values)
    }

    pub(crate) fn bootstrap_values(&self) -> RunConnectionValues {
        self.values.as_ref().clone()
    }

    pub(crate) async fn resolve_source(
        &self,
        fields: BTreeSet<EnvironmentVariableName>,
    ) -> Result<Option<StaticConnectionValues>, RunEnvironmentError> {
        self.source_authorization.validate_source_fields(&fields)?;
        let Some(key) = self.source_connection.clone() else {
            return Ok(None);
        };
        let mut resolved = self
            .resolve_requirements(BTreeMap::from([(key.clone(), fields)]))
            .await?;
        resolved
            .remove(&key)
            .map(Some)
            .ok_or(RunEnvironmentError::MissingConnection(key))
    }

    pub(super) async fn resolve(
        &self,
        binding: &NodeRuntimeBinding,
    ) -> Result<ResolvedEnvironment, RunEnvironmentError> {
        let resolved = self.resolve_snapshot(binding).await?;
        let dynamic = self.requires_refresh(binding);
        if !dynamic {
            return Ok(resolved);
        }
        Ok(with_environment_refresh(
            resolved,
            Arc::new(NodeEnvironmentRefresh {
                environment: self.clone(),
                binding: binding.clone(),
            }),
        ))
    }

    fn requires_refresh(&self, binding: &NodeRuntimeBinding) -> bool {
        let dynamic_node = binding
            .declared_connections()
            .iter()
            .any(|(key, _)| self.dynamic_keys.contains(key));
        let dynamic_source = matches!(binding, NodeRuntimeBinding::GitDelivery { .. })
            && self
                .source_connection
                .as_ref()
                .is_some_and(|key| self.dynamic_keys.contains(key));
        dynamic_node || dynamic_source
    }

    async fn resolve_snapshot(
        &self,
        binding: &NodeRuntimeBinding,
    ) -> Result<ResolvedEnvironment, RunEnvironmentError> {
        let requirements = declared_requirements(binding.declared_connections());
        let resolved = self.resolve_requirements(requirements).await?;
        let mut values = BTreeMap::new();
        for (key, fields) in binding.declared_connections().iter() {
            let source = resolved
                .get(key)
                .ok_or_else(|| RunEnvironmentError::MissingConnection(key.clone()))?;
            for name in fields.iter() {
                let value =
                    source.as_map().get(name).cloned().ok_or_else(|| {
                        RunEnvironmentError::MissingField(key.clone(), name.clone())
                    })?;
                if values.insert(name.clone(), value).is_some() {
                    return Err(RunEnvironmentError::InvalidPlan);
                }
            }
        }
        let resolved = ResolvedEnvironment::exact(binding, values)
            .map_err(|_| RunEnvironmentError::InvalidPlan)?;
        let resolved = resolved.with_variables(self.variables.as_ref());
        if matches!(binding, NodeRuntimeBinding::GitDelivery { .. }) {
            Ok(resolved.with_source_git_credentials(self.resolve_git_source().await?))
        } else {
            Ok(resolved)
        }
    }

    async fn resolve_requirements(
        &self,
        requirements: BTreeMap<ConnectionKey, BTreeSet<EnvironmentVariableName>>,
    ) -> Result<RunConnectionValues, RunEnvironmentError> {
        validate_requested_fields(self.requirements.as_ref(), &requirements)?;
        let mut selected = RunConnectionValues::new();
        let mut dynamic = RunConnectionRequirements::new();
        for (key, fields) in requirements {
            if let Some(values) = self.values.get(&key) {
                selected.insert(key.clone(), select_required(&key, &fields, values)?);
            } else if self.dynamic_keys.contains(&key) {
                dynamic.insert(key, fields.into_iter().collect());
            } else {
                return Err(RunEnvironmentError::MissingConnection(key));
            }
        }
        selected.extend(self.resolve_dynamic_values(dynamic).await?);
        validate_size(&selected)?;
        Ok(selected)
    }

    async fn resolve_dynamic_values(
        &self,
        requirements: RunConnectionRequirements,
    ) -> Result<RunConnectionValues, RunEnvironmentError> {
        if requirements.is_empty() {
            return Ok(RunConnectionValues::new());
        }
        let resolver = self
            .resolver
            .as_ref()
            .ok_or(RunEnvironmentError::InvalidPlan)?;
        let resolved = resolver
            .resolve(requirements.clone())
            .await
            .map_err(RunEnvironmentError::from)?;
        validate_resolution(&requirements, &resolved)?;
        Ok(resolved)
    }
}

type ConnectionRequirements = BTreeMap<ConnectionKey, BTreeSet<EnvironmentVariableName>>;

#[derive(Clone)]
enum SourceAuthorization {
    Legacy,
    None,
    Git {
        key: ConnectionKey,
        fields: BTreeSet<EnvironmentVariableName>,
    },
}

impl SourceAuthorization {
    fn from_source(source: Option<&ResolvedSource>) -> Self {
        let Some(source) = source else {
            return Self::None;
        };
        let Some(remote) = &source.remote else {
            return Self::Legacy;
        };
        let Some((key, fields)) = remote.connection_requirements().into_iter().next() else {
            return Self::None;
        };
        Self::Git { key, fields }
    }

    fn requirements(&self) -> ConnectionRequirements {
        match self {
            Self::Git { key, fields } => BTreeMap::from([(key.clone(), fields.clone())]),
            Self::Legacy | Self::None => BTreeMap::new(),
        }
    }

    fn connection(&self, legacy: Option<ConnectionKey>) -> Option<ConnectionKey> {
        match self {
            Self::Legacy => legacy,
            Self::None => None,
            Self::Git { key, .. } => Some(key.clone()),
        }
    }

    fn validate_resolver(
        &self,
        dynamic_keys: &BTreeSet<ConnectionKey>,
        source_connection: Option<&ConnectionKey>,
    ) -> Result<(), RunEnvironmentError> {
        let expected = match self {
            Self::Legacy => return Ok(()),
            Self::None => None,
            Self::Git { key, .. } => dynamic_keys.contains(key).then_some(key),
        };
        if source_connection != expected {
            return Err(RunEnvironmentError::InvalidPlan);
        }
        Ok(())
    }

    fn validate_source_fields(
        &self,
        requested: &BTreeSet<EnvironmentVariableName>,
    ) -> Result<(), RunEnvironmentError> {
        if let Self::Git { fields, .. } = self {
            if fields != requested {
                return Err(RunEnvironmentError::InvalidPlan);
            }
        }
        Ok(())
    }
}

struct EnvironmentAdmission {
    requirements: ConnectionRequirements,
    variables: Arc<BTreeMap<EnvironmentVariableName, String>>,
    source: SourceAuthorization,
}

impl EnvironmentAdmission {
    fn new(runtime: &RuntimePlan, definition: Option<&RuntimeEnvironment>) -> Self {
        Self {
            requirements: run_connection_requirements(runtime, definition),
            variables: runtime_variables(definition),
            source: SourceAuthorization::Legacy,
        }
    }

    fn for_submission(submission: &RunSubmission) -> Self {
        Self::new(&submission.runtime, submission.environment.as_ref())
            .with_source(submission.source.as_ref())
    }

    fn for_admitted(admitted: &AdmittedRun) -> Self {
        Self::new(&admitted.runtime, admitted.environment.as_ref())
            .with_source(admitted.source.as_ref())
    }

    fn with_source(self, source: Option<&ResolvedSource>) -> Self {
        self.with_authorization(SourceAuthorization::from_source(source))
    }

    fn with_authorization(mut self, authorization: SourceAuthorization) -> Self {
        self.source = authorization;
        for (key, fields) in self.source.requirements() {
            self.requirements.entry(key).or_default().extend(fields);
        }
        self
    }
}

fn validate_dynamic_plan(dynamic: &DynamicConnectionPlan) -> Result<(), RunEnvironmentError> {
    if dynamic.keys.is_empty()
        || dynamic
            .source_connection
            .as_ref()
            .is_some_and(|key| !dynamic.keys.contains(key))
    {
        return Err(RunEnvironmentError::InvalidPlan);
    }
    Ok(())
}

fn validate_requested_fields(
    authorized: &ConnectionRequirements,
    requested: &ConnectionRequirements,
) -> Result<(), RunEnvironmentError> {
    for (key, fields) in requested {
        let permitted = authorized
            .get(key)
            .ok_or_else(|| RunEnvironmentError::UndeclaredConnection(key.clone()))?;
        if let Some(field) = fields.iter().find(|field| !permitted.contains(*field)) {
            return Err(RunEnvironmentError::UndeclaredField(
                key.clone(),
                field.clone(),
            ));
        }
    }
    Ok(())
}

fn runtime_variables(
    definition: Option<&RuntimeEnvironment>,
) -> Arc<BTreeMap<EnvironmentVariableName, String>> {
    Arc::new(
        definition
            .map(|value| value.variables.clone())
            .unwrap_or_default(),
    )
}

fn declared_requirements(
    connections: &openengine_cluster_protocol::DeclaredConnections,
) -> BTreeMap<ConnectionKey, BTreeSet<EnvironmentVariableName>> {
    connections
        .iter()
        .map(|(key, fields)| (key.clone(), fields.as_set().clone()))
        .collect()
}

#[derive(Clone)]
struct NodeEnvironmentRefresh {
    environment: RunEnvironment,
    binding: NodeRuntimeBinding,
}

#[async_trait]
impl RuntimeEnvironmentRefresh for NodeEnvironmentRefresh {
    async fn refresh(&self) -> Result<ResolvedEnvironment, EnvironmentRefreshError> {
        let resolved = self
            .environment
            .resolve_snapshot(&self.binding)
            .await
            .map_err(EnvironmentRefreshError::from)?;
        Ok(with_environment_refresh(resolved, Arc::new(self.clone())))
    }
}

fn validate_bootstrap(
    declared: &ConnectionRequirements,
    values: &RunConnectionValues,
    dynamic_keys: &BTreeSet<ConnectionKey>,
) -> Result<(), RunEnvironmentError> {
    if let Some(key) = declared
        .keys()
        .find(|key| !values.contains_key(*key) && !dynamic_keys.contains(*key))
    {
        return Err(RunEnvironmentError::MissingConnection(key.clone()));
    }
    if let Some(key) = values
        .keys()
        .chain(dynamic_keys.iter())
        .find(|key| !declared.contains_key(*key))
    {
        return Err(RunEnvironmentError::UndeclaredConnection(key.clone()));
    }
    if let Some(key) = values.keys().find(|key| dynamic_keys.contains(*key)) {
        return Err(RunEnvironmentError::UndeclaredConnection(key.clone()));
    }
    for (key, supplied) in values {
        let fields = declared
            .get(key)
            .ok_or_else(|| RunEnvironmentError::UndeclaredConnection(key.clone()))?;
        validate_connection_shape(key, fields, supplied)?;
    }
    validate_size(values)
}

fn validate_resolution(
    requirements: &RunConnectionRequirements,
    resolved: &RunConnectionValues,
) -> Result<(), RunEnvironmentError> {
    if let Some(key) = requirements.keys().find(|key| !resolved.contains_key(*key)) {
        return Err(RunEnvironmentError::MissingConnection(key.clone()));
    }
    if let Some(key) = resolved.keys().find(|key| !requirements.contains_key(*key)) {
        return Err(RunEnvironmentError::UndeclaredConnection(key.clone()));
    }
    for (key, fields) in requirements {
        let fields = fields.iter().cloned().collect::<BTreeSet<_>>();
        let values = resolved
            .get(key)
            .ok_or_else(|| RunEnvironmentError::MissingConnection(key.clone()))?;
        validate_connection_shape(key, &fields, values)?;
    }
    Ok(())
}

fn select_required(
    key: &ConnectionKey,
    fields: &BTreeSet<EnvironmentVariableName>,
    values: &StaticConnectionValues,
) -> Result<StaticConnectionValues, RunEnvironmentError> {
    let selected = fields
        .iter()
        .map(|name| {
            values
                .as_map()
                .get(name)
                .cloned()
                .map(|value| (name.clone(), value))
                .ok_or_else(|| RunEnvironmentError::MissingField(key.clone(), name.clone()))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    StaticConnectionValues::new(selected).map_err(|_| RunEnvironmentError::InvalidPlan)
}

fn select_environment_values<'a>(
    key: ConnectionKey,
    names: impl IntoIterator<Item = &'a EnvironmentVariableName>,
    available: &BTreeMap<EnvironmentVariableName, String>,
) -> Result<BTreeMap<EnvironmentVariableName, String>, RunEnvironmentError> {
    let mut selected = BTreeMap::new();
    for name in names {
        let value = available
            .get(name)
            .cloned()
            .ok_or_else(|| RunEnvironmentError::MissingField(key.clone(), name.clone()))?;
        selected.insert(name.clone(), value);
    }
    Ok(selected)
}

fn validate_connection_shape(
    key: &ConnectionKey,
    fields: &BTreeSet<EnvironmentVariableName>,
    supplied: &StaticConnectionValues,
) -> Result<(), RunEnvironmentError> {
    if let Some(name) = fields
        .iter()
        .find(|name| !supplied.as_map().contains_key(*name))
    {
        return Err(RunEnvironmentError::MissingField(key.clone(), name.clone()));
    }
    if let Some(name) = supplied
        .as_map()
        .keys()
        .find(|name| !fields.contains(*name))
    {
        return Err(RunEnvironmentError::UndeclaredField(
            key.clone(),
            name.clone(),
        ));
    }
    Ok(())
}

fn validate_size(values: &RunConnectionValues) -> Result<(), RunEnvironmentError> {
    let total = values.iter().try_fold(0_usize, |total, (key, values)| {
        total
            .checked_add(connection_size(key, values)?)
            .ok_or(RunEnvironmentError::TooLarge)
    })?;
    if total > MAX_RUN_ENVIRONMENT_BYTES {
        return Err(RunEnvironmentError::TooLarge);
    }
    Ok(())
}

fn connection_size(
    key: &ConnectionKey,
    values: &StaticConnectionValues,
) -> Result<usize, RunEnvironmentError> {
    values
        .as_map()
        .iter()
        .try_fold(0_usize, |total, (name, value)| {
            total
                .checked_add(key.as_str().len())
                .and_then(|size| size.checked_add(name.as_str().len()))
                .and_then(|size| size.checked_add(value.len()))
                .ok_or(RunEnvironmentError::TooLarge)
        })
}

impl fmt::Debug for RunEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunEnvironment")
            .field(
                "connections",
                &self
                    .values
                    .iter()
                    .map(|(key, values)| (key, values.field_names()))
                    .collect::<BTreeMap<_, _>>(),
            )
            .field("dynamic_keys", &self.dynamic_keys)
            .field("source_connection", &self.source_connection)
            .field("values", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum RunEnvironmentError {
    #[error("declared connection {0} is unavailable")]
    MissingConnection(ConnectionKey),
    #[error("connection {0} field {1} is unavailable")]
    MissingField(ConnectionKey, EnvironmentVariableName),
    #[error("connection {0} was not declared by the run")]
    UndeclaredConnection(ConnectionKey),
    #[error("connection {0} field {1} was not declared by the run")]
    UndeclaredField(ConnectionKey, EnvironmentVariableName),
    #[error("dynamic connection resolution is unavailable")]
    ResolutionUnavailable,
    #[error("dynamic connection resolution was refused")]
    ResolutionRefused,
    #[error("dynamic connection resolution returned an invalid response")]
    ResolutionInvalid,
    #[error("run environment exceeds the aggregate bound")]
    TooLarge,
    #[error("run runtime plan is inconsistent")]
    InvalidPlan,
}

impl From<ConnectionResolutionError> for RunEnvironmentError {
    fn from(error: ConnectionResolutionError) -> Self {
        match error {
            ConnectionResolutionError::Unavailable => Self::ResolutionUnavailable,
            ConnectionResolutionError::Refused => Self::ResolutionRefused,
            ConnectionResolutionError::InvalidResponse => Self::ResolutionInvalid,
        }
    }
}

impl From<RunEnvironmentError> for EnvironmentRefreshError {
    fn from(error: RunEnvironmentError) -> Self {
        match error {
            RunEnvironmentError::ResolutionUnavailable => Self::Unavailable,
            RunEnvironmentError::ResolutionRefused => Self::Refused,
            _ => Self::InvalidResponse,
        }
    }
}

#[cfg(test)]
#[path = "environment/tests.rs"]
mod tests;
