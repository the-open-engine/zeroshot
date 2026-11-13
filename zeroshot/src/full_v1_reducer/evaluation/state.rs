use super::super::*;

pub(super) struct LoopCompletion {
    pub(super) local: Context,
    pub(super) position: HistoryPosition,
    pub(super) label: &'static str,
    pub(super) iteration: u64,
}

impl LoopCompletion {
    pub(super) fn new(
        local: Context,
        position: HistoryPosition,
        label: &'static str,
        iteration: u64,
    ) -> Self {
        Self {
            local,
            position,
            label,
            iteration,
        }
    }
}

pub(super) enum VisitResolution<'history> {
    Exhausted,
    Ready(Box<ExecutableVisit<'history>>),
}

pub(super) struct LoopFinishRequest<'request, 'traversal> {
    pub(super) group: &'request openengine_cluster_protocol::LoopNode,
    pub(super) context: &'request mut Context,
    pub(super) traversal: Traversal<'traversal>,
    pub(super) completion: LoopCompletion,
}

pub(super) struct ExistingExecutionRequest<'graph, 'context, 'traversal, 'history> {
    pub(super) spec: ExecutableSpec<'graph>,
    pub(super) context: &'context mut Context,
    pub(super) traversal: Traversal<'traversal>,
    pub(super) execution: &'history DurableExecution,
    pub(super) input: Value,
}

pub(super) struct MissingDispatchRequest<'graph, 'context, 'traversal, 'history> {
    pub(super) spec: ExecutableSpec<'graph>,
    pub(super) visit: ExecutableVisit<'history>,
    pub(super) context: &'context Context,
    pub(super) traversal: Traversal<'traversal>,
}

pub(super) struct RetryDispatchRequest<'graph, 'traversal, 'history> {
    pub(super) spec: ExecutableSpec<'graph>,
    pub(super) traversal: Traversal<'traversal>,
    pub(super) execution: &'history DurableExecution,
    pub(super) input: Value,
}

pub(super) struct OutcomeApplication<'request, 'graph> {
    pub(super) spec: &'request ExecutableSpec<'graph>,
    pub(super) context: &'request mut Context,
    pub(super) map_indices: &'request [u64],
    pub(super) outcome: &'request WorkerOutcome,
}

pub(super) struct ExecutableVisit<'history> {
    pub(super) occurrence: StructuralOccurrence,
    pub(super) node_instance: Option<NodeInstanceId>,
    pub(super) executions: Vec<&'history DurableExecution>,
    pub(super) attempt: PositiveInteger,
    pub(super) existing: Option<&'history DurableExecution>,
}

pub(super) fn attempts_exhausted() -> Result<Status, ReducerError> {
    Ok(Status::Terminal {
        position: HistoryPosition::ZERO,
        projection: TerminalProjection::Failed {
            context: None,
            reason: attempts_exhausted_reason()?,
        },
    })
}

pub(super) struct DispatchVisit {
    pub(super) node_instance: NodeInstanceId,
    pub(super) occurrence: StructuralOccurrence,
    pub(super) attempt: PositiveInteger,
}
