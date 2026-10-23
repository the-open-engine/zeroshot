use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use openengine_cluster_server::graph_verifier::FULL_V1_MAX_NODE_EXECUTIONS;

use super::*;
use crate::execution::process::HostedProcessPool;
use crate::native_v2_capsule::provider_process::process_scope;

struct ContainedIdentityDriver {
    inner: FakeDriver,
    pool: HostedProcessPool,
    homes: StdMutex<BTreeSet<PathBuf>>,
}

#[async_trait]
impl NodeDriver for ContainedIdentityDriver {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        let scope = process_scope(&invocation)?;
        // Exercise the real contained identity conversion without starting a model process.
        self.pool
            .identity(scope)
            .map_err(|error| NodeRunnerError::DriverDetail(error.to_string()))?;
        self.homes
            .lock()
            .assert_value()
            .insert(scope.private_home(Path::new("/runtime")));
        self.inner.run(invocation, control).await
    }
}

#[tokio::test]
async fn contained_auxiliary_sessions_start_after_the_last_primary_identity() {
    let admitted = admitted();
    let pool = HostedProcessPool::new(10_002, 10_002, 20_000)
        .assert_value()
        .active_run_slot(0, FULL_V1_MAX_NODE_EXECUTIONS * 2)
        .assert_value();
    let driver = Arc::new(ContainedIdentityDriver {
        inner: FakeDriver::default(),
        pool,
        homes: StdMutex::default(),
    });
    let runner = NativeNodeRunner::new(&admitted, driver.clone(), Arc::new(FakeFactory::default()))
        .assert_value();
    runner
        .start(request(
            "contained",
            "verify",
            (FULL_V1_MAX_NODE_EXECUTIONS, FULL_V1_MAX_NODE_EXECUTIONS),
        ))
        .await
        .assert_value()
        .completion()
        .await
        .assert_value();
    let factory = runner.auxiliary_factory();
    let global = factory.fork(&admitted).assert_value();
    for (index, node) in ["left", "right", "slow_reuse", "verify"]
        .into_iter()
        .enumerate()
    {
        let identity = AUXILIARY_PROVIDER_SLOT_OFFSET + index as u64 + 1;
        global
            .start(request("contained", node, (identity, identity)))
            .await
            .assert_value()
            .completion()
            .await
            .assert_value();
    }
    factory.close_run(&RunId::new("contained")).await;
    runner.close_run(&RunId::new("contained")).await;
    assert_eq!(driver.homes.lock().assert_value().len(), 5);
    let slots = driver.inner.slots.lock().assert_value();
    assert_eq!(slots.len(), 5);
    assert!(
        slots
            .iter()
            .skip(1)
            .all(|(_, slot)| *slot > FULL_V1_MAX_NODE_EXECUTIONS)
    );
}
