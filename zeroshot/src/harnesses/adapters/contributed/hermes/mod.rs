//! Community-maintained Hermes adapter using its native Python agent and configuration factories.
//! The canonical executable supplies its own interpreter; no gateway, profile files or plugins are
//! replaced. One private runtime owns each execution and resumes the native node session database.

mod command;
mod connection;
mod registration;

pub(crate) use registration::registration;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use openengine_cluster_protocol::WorkerOutcome;

use crate::execution::driver::DriverCancellation;
use crate::execution::process::HostedProcessPool;
use crate::native_v2_capsule::provider_process::{
    ClosedSessionFailure, LocalHarnessEnvironment, ProviderExecution, ProviderFilesystemConfig,
    ProviderProcessRunners, ProviderSessionCore, impl_provider_node_session,
    open_provider_process_with_cancellation, provider_redactions, redact_provider_error,
};
use crate::native_v2_runner::{DriverControl, DriverInvocation, NodeDriver, NodeRunnerError};

pub struct HermesConfig {
    pub provider: String,
    pub executable: String,
    pub prefix_arguments: Vec<String>,
    pub workspace: PathBuf,
    pub runtime_home: PathBuf,
    pub local_user_home: Option<PathBuf>,
    pub native_environment: LocalHarnessEnvironment,
    pub base_environment: crate::native_v2_capsule::HarnessProcessEnvironment,
    pub process_pool: HostedProcessPool,
}

pub struct HermesAdapter {
    config: HermesConfig,
    runners: ProviderProcessRunners,
    local_environment: BTreeMap<String, String>,
    active_memory: Option<Arc<crate::active_memory::hooks::ActiveMemoryHooks>>,
}

crate::active_memory::hooks::impl_active_memory_adapter!(HermesAdapter);

impl HermesAdapter {
    #[must_use]
    pub fn new(mut config: HermesConfig) -> Self {
        config.local_user_home = None;
        config.native_environment = LocalHarnessEnvironment::default();
        let runners = ProviderProcessRunners::hosted(config.process_pool);
        Self {
            config,
            runners,
            local_environment: BTreeMap::new(),
            active_memory: None,
        }
    }

    #[must_use]
    pub fn new_local(config: HermesConfig) -> Self {
        let local_environment = config
            .native_environment
            .selected(command::LOCAL_ENVIRONMENT);
        Self {
            config,
            runners: ProviderProcessRunners::local(),
            local_environment,
            active_memory: None,
        }
    }

    async fn run_session(
        &self,
        invocation: &DriverInvocation,
        session: &HermesSession,
        control: &DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let _turn = session.core.turn.lock().await;
        session
            .core
            .ensure_live(ClosedSessionFailure::SessionLost)?;
        let execution = ProviderExecution::new(
            ProviderFilesystemConfig {
                runners: self.runners,
                root: &self.config.runtime_home,
                workspace: &self.config.workspace,
            },
            invocation,
            &session.core,
        );
        let files = execution
            .prepare(control)
            .await
            .map_err(connection::process_error)?;
        let (command, init) = command::prepare(self, invocation, &files, &session.id)?;
        let redactions = self.redactions(&invocation.environment);
        let (kill, cancellation) = tokio::sync::watch::channel(false);
        // Native terminal jobs can detach from the provider's process group. Ask Hermes to stop
        // and reap those jobs before permitting the ordinary bounded process kill fallback.
        let mut process = open_provider_process_with_cancellation(
            files,
            command,
            control,
            DriverCancellation::new(cancellation),
        )
        .await?
        .map_err(connection::process_error)?;
        let mut connection =
            connection::HermesConnection::new(process.detach_stdout(), control, redactions.clone());
        let result = connection.execute(&process, invocation, init).await;
        connection
            .finish(&mut process, kill, result)
            .await
            .map_err(|error| redact_provider_error(error, &redactions))
    }

    fn redactions(
        &self,
        environment: &crate::native_v2_runner::ResolvedEnvironment,
    ) -> Vec<String> {
        let mut values = provider_redactions(environment, &self.local_environment);
        values.extend(self.local_environment.get("ANTHROPIC_TOKEN").cloned());
        values
    }
}

struct HermesSession {
    core: ProviderSessionCore,
    id: String,
}

impl_provider_node_session!(HermesSession);

crate::native_v2_capsule::provider_process::impl_provider_session_factory!(
    HermesAdapter,
    HermesSession {
        core: ProviderSessionCore::new(),
        id: uuid::Uuid::now_v7().to_string()
    }
);

#[async_trait]
impl NodeDriver for HermesAdapter {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let invocation = self.memory_invocation(invocation);
        let session = crate::native_v2_capsule::provider_process::invocation_session::<
            HermesSession,
        >(&invocation)?;
        let redactions = self.redactions(&invocation.environment);
        self.run_session(&invocation, session, &control)
            .await
            .map_err(|error| redact_provider_error(error, &redactions))
    }
}

#[cfg(all(test, unix))]
mod tests;
