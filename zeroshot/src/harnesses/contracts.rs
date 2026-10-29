use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::execution::process::HostedProcessPool;
use crate::active_memory::hooks::ActiveMemoryHooks;
use crate::native_v2_capsule::provider_process::LocalHarnessEnvironment;
use crate::native_v2_contract::RuntimeLane;
use crate::native_v2_runner::{NodeDriver, SessionFactory};

/// A host supplies local native state only to a local adapter.
#[derive(Clone)]
pub enum HarnessHost {
    Local {
        home: Option<PathBuf>,
        environment: LocalHarnessEnvironment,
    },
    Contained {
        process_pool: HostedProcessPool,
    },
}

impl HarnessHost {
    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }
}

/// Private launch inputs, never serialized into a profile, ledger or observation.
#[derive(Clone)]
pub struct HarnessBuildContext {
    pub lane: RuntimeLane,
    pub workspace: PathBuf,
    pub runtime_home: PathBuf,
    pub executable: PathBuf,
    pub prefix_arguments: Vec<String>,
    pub search_path: String,
    pub base_environment: BTreeMap<String, String>,
    pub host: HarnessHost,
}

pub struct HarnessAdapter {
    pub driver: Arc<dyn NodeDriver>,
    pub sessions: Arc<dyn SessionFactory>,
}

impl HarnessAdapter {
    pub fn new<T: NodeDriver + SessionFactory + 'static>(adapter: T) -> Self {
        let adapter = Arc::new(adapter);
        Self {
            driver: adapter.clone(),
            sessions: adapter,
        }
    }
}

/// Configuration uses the same factory boundary for every registered harness.
pub struct HarnessConfig {
    pub(crate) context: HarnessBuildContext,
    pub(super) lane: RuntimeLane,
    pub(crate) build: fn(
        HarnessBuildContext,
        Option<Arc<ActiveMemoryHooks>>,
    ) -> Result<HarnessAdapter, HarnessError>,
}

impl HarnessConfig {
    #[must_use]
    pub fn lane(&self) -> RuntimeLane {
        self.lane.clone()
    }
    #[must_use]
    pub fn workspace(&self) -> &std::path::Path {
        &self.context.workspace
    }
    pub(crate) fn build(
        self,
        memory: Option<Arc<ActiveMemoryHooks>>,
    ) -> Result<HarnessAdapter, HarnessError> {
        if self.context.lane != self.lane {
            return Err(HarnessError::Configuration(
                "configured harness lane was changed".to_owned(),
            ));
        }
        (self.build)(self.context, memory)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum HarnessError {
    #[error(transparent)]
    Value(#[from] openengine_cluster_protocol::NativeV2RunValueError),
    #[error("harness `{0}` is not enabled on this host")]
    UnknownHarness(String),
    #[error("harness/provider lane {0} is incompatible")]
    UnsupportedProvider(RuntimeLane),
    #[error("duplicate harness registration `{0}`")]
    Duplicate(String),
    #[error("harness configuration is invalid: {0}")]
    Configuration(String),
}

#[derive(Clone, Copy)]
pub(crate) struct ProviderAccessContract {
    pub(crate) native_local: bool,
    pub(crate) connection_key: &'static str,
    pub(crate) required_fields: &'static [&'static str],
    pub(crate) accepted_field_sets: &'static [&'static [&'static str]],
}

impl ProviderAccessContract {
    pub(crate) const fn new(
        native_local: bool,
        connection_key: &'static str,
        required_fields: &'static [&'static str],
        accepted_field_sets: &'static [&'static [&'static str]],
    ) -> Self {
        Self {
            native_local,
            connection_key,
            required_fields,
            accepted_field_sets,
        }
    }
}
