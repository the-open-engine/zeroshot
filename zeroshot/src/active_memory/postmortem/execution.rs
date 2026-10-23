use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, stream::FuturesUnordered};
use openengine_cluster_protocol::{NodeName, WorkerOutcome};
use serde_json::Value;
use tokio::sync::watch;

use super::plan::Role;
use super::proposals::ProposalContext;
use super::{PostmortemError, PostmortemReport, PostmortemRequest};
use crate::active_memory::LessonMutation;
use crate::native_v2_contract::{
    AdmittedRun, ExecutionId, ExecutionRef, NodeInstanceId, NodeInvocation,
};
use crate::native_v2_runner::{
    AttachReceiveError, DurableNodeEvent, DurableOutput, LiveOutputStream, NodeHandle,
    NodeRunRequest, NodeRunner, NodeRunnerError, AUXILIARY_PROVIDER_SLOT_OFFSET, bounded_log_text,
    current_timestamp,
};
use crate::v2_run_ledger::{RunEvent, SafeLogLine, SafeLogStream, MAX_SAFE_LOG_BYTES};

pub(super) struct PostmortemExecution<'a> {
    pub(super) runner: Arc<dyn NodeRunner>,
    pub(super) plan: &'a AdmittedRun,
    pub(super) request: &'a PostmortemRequest,
    pub(super) context: &'a ProposalContext<'a>,
    pub(super) cancellation: watch::Receiver<bool>,
}

pub(super) async fn run(
    execution: PostmortemExecution<'_>,
) -> Result<(PostmortemReport, Vec<LessonMutation>), PostmortemError> {
    let (stop, stopping) = watch::channel(false);
    let mut cancellation = execution.cancellation.clone();
    let mut work = FuturesUnordered::new();
    for (index, role) in Role::ALL.into_iter().enumerate() {
        if execution.context.role_enabled(role) {
            work.push(run_role(&execution, role, index, stopping.clone()));
        }
    }
    let deadline = tokio::time::sleep(Duration::from_millis(u64::from(
        execution
            .request
            .admitted
            .runtime
            .global_agents()
            .postmortem
            .timeout_ms,
    )));
    tokio::pin!(deadline);
    let mut report = PostmortemReport::default();
    let mut mutations = BTreeMap::new();
    let mut cleanup_failed = false;
    while !work.is_empty() {
        tokio::select! {
            biased;
            _ = wait_cancelled(&mut cancellation), if !report.cancelled => {
                report.cancelled=true; stop.send_replace(true);
            },
            () = &mut deadline, if !report.cancelled => {
                report.cancelled=true; stop.send_replace(true);
            },
            result = work.next() => if let Some(result)=result {
                match result {
                    Ok(proposals) => {
                        report.completed_roles+=1;
                        for proposal in proposals { mutations.entry(proposal.id().to_owned()).or_insert(proposal); }
                    },
                    Err(PostmortemError::CleanupUnconfirmed) => {
                        cleanup_failed=true; stop.send_replace(true); report.failed_roles+=1;
                    },
                    Err(_) => { report.failed_roles+=1; },
                }
            },
        }
    }
    if cleanup_failed {
        return Err(PostmortemError::CleanupUnconfirmed);
    }
    if report.cancelled {
        mutations.clear();
    }
    Ok((report, mutations.into_values().collect()))
}

async fn run_role(
    execution: &PostmortemExecution<'_>,
    role: Role,
    index: usize,
    mut cancellation: watch::Receiver<bool>,
) -> Result<Vec<LessonMutation>, PostmortemError> {
    let result = run_role_batches(execution, role, index, &mut cancellation).await;
    let status = if result.is_ok() {
        "completed"
    } else {
        "failed"
    };
    let _ = log(
        execution.request,
        format!("Active Memory {} {status}", role.name()),
    )
    .await;
    result
}

struct RoleBatch<'a> {
    role: Role,
    index: usize,
    round: usize,
    input: &'a Value,
}

async fn run_role_batches(
    execution: &PostmortemExecution<'_>,
    role: Role,
    index: usize,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<Vec<LessonMutation>, PostmortemError> {
    let inputs = execution
        .context
        .inputs(role)
        .map_err(|()| PostmortemError::Preparation)?;
    let mut mutations = Vec::new();
    for (round, input) in inputs.iter().enumerate() {
        if inputs.len() > 1 {
            log(
                execution.request,
                format!(
                    "Active Memory {} batch {}/{}",
                    role.name(),
                    round + 1,
                    inputs.len()
                ),
            )
            .await?;
        }
        let batch = RoleBatch {
            role,
            index,
            round,
            input,
        };
        let output = run_role_inner(execution, &batch, cancellation).await?;
        match execution.context.mutations(role, input, output) {
            Ok(proposals) => mutations.extend(proposals),
            Err(()) => {
                let _ = log(execution.request, format!("Active Memory {} findings rejected by scope, evidence, card validation or incomplete reconciliation", role.name())).await;
                return Err(PostmortemError::Preparation);
            }
        }
    }
    Ok(mutations)
}

async fn run_role_inner(
    execution: &PostmortemExecution<'_>,
    batch: &RoleBatch<'_>,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<Value, PostmortemError> {
    if *cancellation.borrow() {
        return Err(PostmortemError::Preparation);
    }
    log(
        execution.request,
        format!("Active Memory {} started", batch.role.name()),
    )
    .await?;
    let invocation = invocation(execution, batch)?;
    let environment = tokio::select! {
        biased;
        ()=wait_cancelled(cancellation)=>return Err(PostmortemError::Preparation),
        result=execution.request.environment.resolve(&invocation.binding)=>result.map_err(|_|PostmortemError::Preparation)?,
    };
    if *cancellation.borrow() {
        return Err(PostmortemError::Preparation);
    }
    let request = NodeRunRequest {
        invocation,
        environment,
    };
    let mut handle = tokio::select! {
        biased;
        result=execution.runner.start(request)=>result.map_err(PostmortemError::from)?,
        ()=wait_cancelled(cancellation)=>return Err(PostmortemError::Preparation),
    };
    let Some(output) = handle.take_initial_output() else {
        handle.cancel();
        let completion = handle.completion().await;
        if completion == Err(NodeRunnerError::CleanupUnconfirmed) {
            return Err(PostmortemError::CleanupUnconfirmed);
        }
        return Err(PostmortemError::Preparation);
    };
    let completion = observe(
        &mut handle,
        bridge(execution.request, batch.role, output),
        cancellation,
    )
    .await?;
    match completion.outcome {
        WorkerOutcome::Verifier { output, .. } => Ok(output),
        _ => Err(PostmortemError::Preparation),
    }
}

fn invocation(
    execution: &PostmortemExecution<'_>,
    batch: &RoleBatch<'_>,
) -> Result<NodeInvocation, PostmortemError> {
    let node = NodeName::new(batch.role.name()).map_err(|_| PostmortemError::Preparation)?;
    let binding = execution
        .plan
        .runtime
        .nodes()
        .get(&node)
        .cloned()
        .ok_or(PostmortemError::Preparation)?;
    let leaf = find_leaf(&execution.plan.graph.root, &node).ok_or(PostmortemError::Preparation)?;
    let number = AUXILIARY_PROVIDER_SLOT_OFFSET
        + u64::try_from(batch.index).map_err(|_| PostmortemError::Preparation)?
        + 1;
    let execution_number = u64::try_from(batch.round)
        .ok()
        .and_then(|round| round.checked_mul(Role::ALL.len() as u64))
        .and_then(|offset| number.checked_add(offset))
        .ok_or(PostmortemError::Preparation)?;
    let input = serde_json::to_string(batch.input).map_err(|_| PostmortemError::Preparation)?;
    if super::context_budget::encoded_size(batch.input) > super::context_budget::MAX_INPUT_BYTES {
        return Err(PostmortemError::Preparation);
    }
    Ok(NodeInvocation {
        reference: ExecutionRef {
            run_id: execution.request.run_id.clone(),
            node,
            node_instance: NodeInstanceId::new(number).map_err(|_| PostmortemError::Preparation)?,
            execution: ExecutionId::new(execution_number)
                .map_err(|_| PostmortemError::Preparation)?,
        },
        worker: leaf.worker.clone(),
        instructions: leaf.instructions.clone(),
        input: serde_json::json!({"context":input}),
        binding,
    })
}

fn find_leaf<'a>(
    node: &'a openengine_cluster_protocol::GraphNode,
    name: &NodeName,
) -> Option<&'a openengine_cluster_protocol::VerifierNode> {
    if let openengine_cluster_protocol::GraphNode::Verifier(leaf) = node {
        if leaf.name == *name {
            return Some(leaf);
        }
    }
    openengine_cluster_server::graph_verifier::graph_node_children(node)
        .into_iter()
        .find_map(|child| find_leaf(child, name))
}

async fn observe(
    handle: &mut NodeHandle,
    events: impl std::future::Future<Output = Result<(), PostmortemError>>,
    cancellation: &mut watch::Receiver<bool>,
) -> Result<crate::native_v2_contract::NodeCompletion, PostmortemError> {
    tokio::pin!(events);
    let mut interrupted = false;
    let mut persisted = None;
    let result = loop {
        tokio::select! {
            completion=handle.completion()=>break completion,
            ()=wait_cancelled(cancellation), if !interrupted=> {interrupted=true;handle.cancel();},
            result=&mut events, if persisted.is_none()=>{
                if result.is_err(){handle.cancel();}
                persisted=Some(result);
            },
        }
    };
    let persistence = match persisted {
        Some(result) => result,
        None => events.await,
    };
    if result == Err(NodeRunnerError::CleanupUnconfirmed) {
        return Err(PostmortemError::CleanupUnconfirmed);
    }
    persistence?;
    if interrupted {
        return Err(PostmortemError::Preparation);
    }
    result.map_err(PostmortemError::from)
}

async fn bridge(
    request: &PostmortemRequest,
    role: Role,
    mut output: DurableOutput,
) -> Result<(), PostmortemError> {
    loop {
        let event = match output.recv().await {
            Ok(event) => event,
            Err(AttachReceiveError::Closed) => return Ok(()),
            Err(AttachReceiveError::Lagged) => return Err(PostmortemError::Storage),
        };
        let event = global_event(role, event)?;
        request
            .ledger
            .append(&request.run_id, vec![event])
            .await
            .map_err(|_| PostmortemError::Storage)?;
    }
}

fn global_event(role: Role, event: DurableNodeEvent) -> Result<RunEvent, PostmortemError> {
    Ok(match event {
        DurableNodeEvent::TokenUsage(usage) => RunEvent::RunTokenUsageObserved { usage },
        DurableNodeEvent::Output { output, timestamp } => RunEvent::SafeLog {
            execution: None,
            timestamp,
            stream: match output.stream {
                LiveOutputStream::Error => SafeLogStream::Error,
                _ => SafeLogStream::System,
            },
            line: SafeLogLine::new(bounded_log_text(
                format!("[{}] {}", role.name(), output.text),
                MAX_SAFE_LOG_BYTES,
            ))
            .map_err(|_| PostmortemError::Storage)?,
            // Auxiliary failures do not establish the primary graph's failure context.
            diagnostic: None,
        },
    })
}

pub(super) async fn log(request: &PostmortemRequest, line: String) -> Result<(), PostmortemError> {
    let line = SafeLogLine::new(line).map_err(|_| PostmortemError::Storage)?;
    request
        .ledger
        .append(
            &request.run_id,
            vec![RunEvent::SafeLog {
                execution: None,
                timestamp: current_timestamp(),
                stream: SafeLogStream::System,
                line,
                diagnostic: None,
            }],
        )
        .await
        .map_err(|_| PostmortemError::Storage)?;
    Ok(())
}

pub(super) async fn record_manifest(
    request: &PostmortemRequest,
    generation: u64,
    supplied: Option<&crate::active_memory::SuppliedMemoryManifest>,
) -> Result<(), PostmortemError> {
    log(request,format!("Active Memory supplied manifest: known={}, snapshot_generation={}, supplied_revisions={}",
        supplied.is_some(),generation,supplied.map_or(0,|manifest|manifest.lessons.len()))).await?;
    let Some(manifest) = supplied else {
        return Ok(());
    };
    let mut chunk = String::from("Active Memory supplied revisions: ");
    for version in &manifest.lessons {
        crate::active_memory::validate_key("supplied lesson ID", &version.id, 96)
            .map_err(|_| PostmortemError::Storage)?;
        let encoded = serde_json::to_string(version).map_err(|_| PostmortemError::Storage)?;
        if chunk.len() + encoded.len() + 1 > MAX_SAFE_LOG_BYTES {
            log(request, chunk).await?;
            chunk = String::from("Active Memory supplied revisions: ");
        }
        chunk.push_str(&encoded);
        chunk.push(' ');
    }
    if !manifest.lessons.is_empty() {
        log(request, chunk).await?;
    }
    Ok(())
}

async fn wait_cancelled(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            return;
        }
    }
}
