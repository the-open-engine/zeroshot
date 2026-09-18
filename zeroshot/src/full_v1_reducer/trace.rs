//! Optional, bounded observation of the existing evaluator. Probe frames are kept private until
//! the owning control operation selects them; they never become extra scheduling decisions.
use super::*;

const MAX_TRACE_RECORDS: usize = 16_384;
const MAX_TRACE_EVALUATIONS: usize = 65_536;
const MAX_TRACE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuralTraceState {
    Entered,
    Completed,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuralTrace {
    pub node: NodeName,
    pub map_indices: Vec<u64>,
    /// One-based iterations of the enclosing loops, outermost first.
    pub loop_iterations: Vec<u64>,
    pub state: StructuralTraceState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<NodeName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TracedReduction {
    pub reduction: Reduction,
    pub trace: Vec<StructuralTrace>,
    pub(crate) evaluations: usize,
}

#[derive(Default)]
pub(super) struct TraceCollector {
    records: Vec<StructuralTrace>,
    evaluations: usize,
    bytes: usize,
}

impl TraceCollector {
    pub(super) fn evaluations(&self) -> usize {
        self.evaluations
    }

    pub(super) fn into_records(self) -> Vec<StructuralTrace> {
        self.records
    }

    fn enter(
        &mut self,
        node: &GraphNode,
        traversal: Traversal<'_>,
    ) -> Result<Option<usize>, ReducerError> {
        self.evaluations += 1;
        if self.evaluations > MAX_TRACE_EVALUATIONS {
            return Err(ReducerError::TraceLimit);
        }
        if matches!(node, GraphNode::Step(_) | GraphNode::Verifier(_)) {
            return Ok(None);
        }
        if self.records.len() >= MAX_TRACE_RECORDS {
            return Err(ReducerError::TraceLimit);
        }
        self.bytes = self.bytes.saturating_add(
            1_024 + 8 * (traversal.map_indices.len() + traversal.loop_iterations.len()),
        );
        if self.bytes > MAX_TRACE_BYTES {
            return Err(ReducerError::TraceLimit);
        }
        let index = self.records.len();
        self.records.push(StructuralTrace {
            node: node.name().clone(),
            map_indices: traversal.map_indices.to_vec(),
            loop_iterations: traversal.loop_iterations.to_vec(),
            state: StructuralTraceState::Entered,
            branch: None,
            detail: None,
            output: None,
        });
        Ok(Some(index))
    }

    fn finish(&mut self, index: usize, status: &Status, context: &Context) {
        let record = &mut self.records[index];
        if !matches!(status, Status::Pending) {
            record.state = StructuralTraceState::Completed;
        }
        record.detail = context
            .controls
            .iter()
            .find(|(key, _)| {
                key.node == record.node
                    && key.source == ControlSource::Group
                    && key.map_indices == record.map_indices
            })
            .map(|(_, label)| label.clone());
    }
}

impl Engine<'_> {
    pub(super) fn trace_enter(
        &mut self,
        node: &GraphNode,
        traversal: Traversal<'_>,
    ) -> Result<Option<usize>, ReducerError> {
        self.trace
            .as_mut()
            .map(|trace| trace.enter(node, traversal))
            .transpose()
            .map(Option::flatten)
    }

    pub(super) fn trace_finish(
        &mut self,
        index: Option<usize>,
        status: &Status,
        context: &Context,
    ) {
        if let (Some(trace), Some(index)) = (&mut self.trace, index) {
            trace.finish(index, status, context);
        }
    }

    pub(super) fn trace_branch(&mut self, node: &NodeName, branch: &NodeName) {
        if let Some(record) = self.trace.as_mut().and_then(|trace| {
            trace
                .records
                .iter_mut()
                .rev()
                .find(|record| &record.node == node)
        }) {
            record.branch = Some(branch.clone());
        }
    }

    pub(super) fn trace_terminal(
        &mut self,
        index: Option<usize>,
        status: &Status,
    ) -> Result<(), ReducerError> {
        let (Some(trace), Some(index), Status::Terminal { projection, .. }) =
            (&mut self.trace, index, status)
        else {
            return Ok(());
        };
        let record = &mut trace.records[index];
        match projection {
            TerminalResult::Succeeded { output } => {
                let mut counter = TraceBytes { bytes: trace.bytes };
                serde_json::to_writer(&mut counter, output)
                    .map_err(|_| ReducerError::TraceLimit)?;
                trace.bytes = counter.bytes;
                record.state = StructuralTraceState::Succeeded;
                record.output = Some(output.clone());
            }
            TerminalResult::Failed { reason } => {
                record.state = StructuralTraceState::Failed;
                record.detail = Some(reason.as_str().to_owned());
            }
        }
        Ok(())
    }

    pub(super) fn trace_checkpoint(&self) -> usize {
        self.trace.as_ref().map_or(0, |trace| trace.records.len())
    }

    pub(super) fn take_trace_since(&mut self, checkpoint: usize) -> Vec<StructuralTrace> {
        self.trace
            .as_mut()
            .map_or_else(Vec::new, |trace| trace.records.split_off(checkpoint))
    }

    pub(super) fn restore_trace(
        &mut self,
        records: Vec<StructuralTrace>,
    ) -> Result<(), ReducerError> {
        if let Some(trace) = &mut self.trace {
            if trace.records.len().saturating_add(records.len()) > MAX_TRACE_RECORDS {
                return Err(ReducerError::TraceLimit);
            }
            trace.records.extend(records);
        }
        Ok(())
    }
}

struct TraceBytes {
    bytes: usize,
}

impl std::io::Write for TraceBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(bytes.len());
        if self.bytes > MAX_TRACE_BYTES {
            return Err(std::io::Error::other("structural trace budget exceeded"));
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
