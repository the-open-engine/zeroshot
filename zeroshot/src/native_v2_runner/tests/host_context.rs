use std::sync::Mutex as StdMutex;

use super::*;

#[derive(Default)]
struct SnapshotContext(AtomicUsize);

impl AgentContextProvider for SnapshotContext {
    fn context(&self) -> String {
        format!(
            "shared capacity snapshot {}",
            self.0.fetch_add(1, Ordering::SeqCst)
        )
    }
}

#[derive(Default)]
struct ContextDriver {
    inner: FakeDriver,
    invocations: StdMutex<Vec<DriverInvocation>>,
}

#[async_trait]
impl NodeDriver for ContextDriver {
    async fn run(
        &self,
        invocation: DriverInvocation,
        control: DriverControl,
    ) -> Result<WorkerOutcome, NodeRunnerError> {
        self.invocations
            .lock()
            .assert_value()
            .push(invocation.clone());
        self.inner.run(invocation, control).await
    }
}

#[tokio::test]
async fn host_context_is_fresh_on_reused_sessions_and_auxiliary_agents() {
    let driver = Arc::new(ContextDriver::default());
    let sessions = Arc::new(FakeFactory::default());
    let context = Arc::new(SnapshotContext::default());
    let runner = NativeNodeRunner::new(&admitted(), driver.clone(), sessions.clone())
        .assert_value()
        .with_agent_context(Some(context.clone()));
    for execution in 1..=2 {
        runner
            .start(request("context", "looped", (1, execution)))
            .await
            .assert_value()
            .completion()
            .await
            .assert_value();
    }
    assert_eq!(sessions.opened.load(Ordering::SeqCst), 1);
    let auxiliary = runner.auxiliary_factory();
    auxiliary
        .fork(&admitted())
        .assert_value()
        .start(request("context", "verify", (2, 3)))
        .await
        .assert_value()
        .completion()
        .await
        .assert_value();
    assert_eq!(context.0.load(Ordering::SeqCst), 3);
    let invocations = driver.invocations.lock().assert_value();
    for (index, invocation) in invocations.iter().enumerate() {
        let context = format!("shared capacity snapshot {index}");
        assert_eq!(invocation.host_context.as_deref(), Some(context.as_str()));
        let instructions = invocation.agent_instructions().assert_value();
        assert!(!instructions.as_str().contains("shared capacity"));
        assert_eq!(invocation.node.input, Value::Null);
        let authored =
            render_agent_prompt(instructions, &invocation.node.input, &invocation.response)
                .assert_value();
        assert_eq!(
            invocation.agent_prompt().assert_value(),
            format!("Runtime-owned execution context:\n{context}\n\n{authored}")
        );
        for dialect in [
            ProviderSchemaDialect::Standard,
            ProviderSchemaDialect::OpenAiStrict,
        ] {
            let authored = render_agent_prompt_with_schema(
                instructions,
                &invocation.node.input,
                &invocation.response,
                dialect,
            )
            .assert_value();
            assert_eq!(
                invocation.agent_prompt_with_schema(dialect).assert_value(),
                format!("Runtime-owned execution context:\n{context}\n\n{authored}")
            );
        }
    }
}

#[tokio::test]
async fn absent_and_empty_host_context_preserve_existing_prompts() {
    let driver = Arc::new(ContextDriver::default());
    let runner = NativeNodeRunner::new(
        &admitted(),
        driver.clone(),
        Arc::new(FakeFactory::default()),
    )
    .assert_value();
    runner
        .start(request("context", "worker", (1, 1)))
        .await
        .assert_value()
        .completion()
        .await
        .assert_value();
    let mut invocation = driver.invocations.lock().assert_value()[0].clone();
    assert!(invocation.host_context.is_none());
    for context in [None, Some(String::new())] {
        invocation.host_context = context;
        assert_eq!(
            invocation.agent_prompt().assert_value(),
            render_agent_prompt(
                invocation.agent_instructions().assert_value(),
                &invocation.node.input,
                &invocation.response,
            )
            .assert_value()
        );
        assert_eq!(
            invocation
                .agent_prompt_with_schema(ProviderSchemaDialect::Standard)
                .assert_value(),
            render_agent_prompt_with_schema(
                invocation.agent_instructions().assert_value(),
                &invocation.node.input,
                &invocation.response,
                ProviderSchemaDialect::Standard,
            )
            .assert_value()
        );
    }
}
