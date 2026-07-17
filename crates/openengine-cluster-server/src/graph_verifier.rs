//! Production semantic verifier for `openengine.graph.full/v1`.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use openengine_cluster_protocol::{
    ChoiceNode, CompiledGraphIr, ControlSelector, ControlSource, DataSelector,
    DiagnosticPathSegment, DiagnosticSeverity, EnumLabel, FieldName, FieldPath, GraphDiagnostic,
    GraphDiagnosticCode, GraphNode, GraphProfile, GraphSpec, Guard, InputBinding, Join, LoopNode,
    MapNode, NodeName, NodeOutputChannel, NodeOutputSelector, NonEmptyVec, ParNode, PayloadType,
    PositiveInteger, SeqNode, StepNode, StructuralBounds, TerminationWitness, VerifierNode,
    WriteBinding,
};

use crate::admission::{GraphVerifier, VerificationError, VerifiedGraph};
use crate::worker_registry::{
    check_graph_workers, WorkerCompatibilityCode, WorkerCompatibilityDiagnostic, WorkerRegistry,
};

pub const FULL_V1_MAX_GRAPH_NODES: u64 = 4_096;
pub const FULL_V1_MAX_GRAPH_DEPTH: u64 = 64;
pub const FULL_V1_MAX_GUARD_NODES: u64 = 4_096;
pub const FULL_V1_MAX_GUARD_ASSIGNMENTS: u64 = 65_536;
pub const FULL_V1_MAX_LOOP_ITERATIONS: u64 = 100;
pub const FULL_V1_MAX_MAP_ITEMS: u64 = 1_024;
pub const FULL_V1_MAX_ATTEMPTS_PER_NODE: u64 = 100;
pub const FULL_V1_MAX_NODE_EXECUTIONS: u64 = 65_536;
pub const FULL_V1_MAX_LOOP_ENTRIES: u64 = 65_536;
pub const FULL_V1_MAX_PEAK_CONCURRENCY: u64 = 1_024;

/// Reusable full-v1 verifier. The registry is the verifier's only asynchronous input.
pub struct ProductionGraphVerifier<R> {
    registry: R,
}

impl<R> ProductionGraphVerifier<R> {
    #[must_use]
    pub const fn new(registry: R) -> Self {
        Self { registry }
    }

    #[must_use]
    pub fn registry(&self) -> &R {
        &self.registry
    }
}

#[async_trait]
impl<R> GraphVerifier for ProductionGraphVerifier<R>
where
    R: WorkerRegistry + 'static,
{
    async fn verify(&self, graph: &GraphSpec) -> Result<VerifiedGraph, VerificationError> {
        let mut analyzer = Analyzer::new(graph);
        let bounds = analyzer.analyze();
        if !analyzer.diagnostics.is_empty() {
            analyzer.sort_diagnostics();
            return Err(VerificationError::Rejected {
                diagnostics: analyzer.diagnostics,
            });
        }

        if let Err(worker_diagnostics) = check_graph_workers(graph, &self.registry).await {
            let mut diagnostics = worker_diagnostics
                .into_iter()
                .map(|diagnostic| analyzer.worker_diagnostic(diagnostic))
                .collect::<Vec<_>>();
            sort_diagnostics(&mut diagnostics);
            return Err(VerificationError::Rejected { diagnostics });
        }

        let bounds = bounds.ok_or_else(|| {
            VerificationError::Internal(
                "structurally valid graph did not produce structural bounds".to_owned(),
            )
        })?;
        finalize_verified(graph, bounds)
    }
}

fn finalize_verified(
    graph: &GraphSpec,
    bounds: StructuralBounds,
) -> Result<VerifiedGraph, VerificationError> {
    finalize_verified_with_invariant_probe(graph, bounds, false)
}

fn finalize_verified_with_invariant_probe(
    graph: &GraphSpec,
    bounds: StructuralBounds,
    inject_invariant_failure: bool,
) -> Result<VerifiedGraph, VerificationError> {
    if inject_invariant_failure {
        return Err(VerificationError::Internal(
            "injected post-validation invariant failure".to_owned(),
        ));
    }
    let compiled_ir = CompiledGraphIr {
        profile: graph.profile,
        initial_input: graph.initial_input.clone(),
        policy: graph.policy.clone(),
        root: graph.root.clone(),
        bounds,
    };
    compiled_ir.canonical_bytes().map_err(|error| {
        VerificationError::Internal(format!(
            "validated graph could not be canonicalized: {error}"
        ))
    })?;
    Ok(VerifiedGraph {
        compiled_ir,
        diagnostics: Vec::new(),
    })
}

#[derive(Clone)]
struct NodeInfo<'a> {
    node: &'a GraphNode,
    path: Vec<DiagnosticPathSegment>,
    ordinal: usize,
}

#[derive(Clone, Default)]
struct Flow {
    available: BTreeSet<NodeName>,
    defined: BTreeMap<FieldPath, PayloadType>,
    failed: BTreeSet<NodeName>,
    outcome_writes: OutcomeWrites,
}

#[derive(Clone, Default)]
struct Effects {
    definite_nodes: BTreeSet<NodeName>,
    definite_writes: Writes,
    possible_writes: Writes,
    possible_write_types: BTreeMap<FieldPath, Vec<PayloadType>>,
    outcome_writes: OutcomeWrites,
    exit_failed: BTreeSet<NodeName>,
    falls_through: bool,
    completion: CompletionPredicate,
}

#[derive(Clone, Default)]
enum CompletionPredicate {
    Always,
    #[default]
    Never,
    Guard(Guard),
    Not(Box<Self>),
    All(Vec<Self>),
    Any(Vec<Self>),
    AtLeast {
        count: u64,
        predicates: Vec<Self>,
    },
}

impl CompletionPredicate {
    fn not(predicate: Self) -> Self {
        match predicate {
            Self::Always => Self::Never,
            Self::Never => Self::Always,
            predicate => Self::Not(Box::new(predicate)),
        }
    }

    fn all(predicates: impl IntoIterator<Item = Self>) -> Self {
        let mut combined = Vec::new();
        for predicate in predicates {
            match predicate {
                Self::Never => return Self::Never,
                Self::Always => {}
                Self::All(nested) => combined.extend(nested),
                predicate => combined.push(predicate),
            }
        }
        match combined.len() {
            0 => Self::Always,
            1 => combined.pop().unwrap_or(Self::Always),
            _ => Self::All(combined),
        }
    }

    fn any(predicates: impl IntoIterator<Item = Self>) -> Self {
        let mut combined = Vec::new();
        for predicate in predicates {
            match predicate {
                Self::Always => return Self::Always,
                Self::Never => {}
                Self::Any(nested) => combined.extend(nested),
                predicate => combined.push(predicate),
            }
        }
        match combined.len() {
            0 => Self::Never,
            1 => combined.pop().unwrap_or(Self::Never),
            _ => Self::Any(combined),
        }
    }

    fn at_least(count: u64, predicates: Vec<Self>) -> Self {
        if count == 0 {
            Self::Always
        } else if count > predicates.len() as u64 {
            Self::Never
        } else if count == predicates.len() as u64 {
            Self::all(predicates)
        } else {
            Self::AtLeast { count, predicates }
        }
    }

    fn evaluate(&self, assignment: &Assignment) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Guard(guard) => evaluate_guard(guard, assignment),
            Self::Not(predicate) => !predicate.evaluate(assignment),
            Self::All(predicates) => predicates
                .iter()
                .all(|predicate| predicate.evaluate(assignment)),
            Self::Any(predicates) => predicates
                .iter()
                .any(|predicate| predicate.evaluate(assignment)),
            Self::AtLeast { count, predicates } => {
                predicates
                    .iter()
                    .filter(|predicate| predicate.evaluate(assignment))
                    .count() as u64
                    >= *count
            }
        }
    }

    fn collect_guards<'a>(&'a self, guards: &mut Vec<&'a Guard>) {
        match self {
            Self::Guard(guard) => guards.push(guard),
            Self::Not(predicate) => predicate.collect_guards(guards),
            Self::All(predicates) | Self::Any(predicates) | Self::AtLeast { predicates, .. } => {
                for predicate in predicates {
                    predicate.collect_guards(guards);
                }
            }
            Self::Always | Self::Never => {}
        }
    }
}

#[derive(Clone)]
struct WriteFact {
    value_type: PayloadType,
    guaranteed_paths: BTreeMap<FieldPath, PayloadType>,
}

struct SelectedOutput {
    value_type: PayloadType,
    definitely_present: bool,
}

type Writes = BTreeMap<FieldPath, WriteFact>;
type OutcomeWrites = BTreeMap<NodeName, Writes>;

impl Flow {
    fn apply_effects(&mut self, effects: &Effects) {
        self.available.extend(effects.definite_nodes.clone());
        apply_write_facts(&mut self.defined, &effects.definite_writes);
        merge_outcome_writes(&mut self.outcome_writes, &effects.outcome_writes);
        self.failed = effects.exit_failed.clone();
        self.resolve_successful_writes();
    }

    fn resolve_successful_writes(&mut self) {
        let successful = self
            .outcome_writes
            .keys()
            .filter(|name| !self.failed.contains(*name))
            .cloned()
            .collect::<Vec<_>>();
        for name in successful {
            if let Some(writes) = self.outcome_writes.remove(&name) {
                apply_write_facts(&mut self.defined, &writes);
            }
        }
    }
}

impl Effects {
    fn resolve_successful_writes(&mut self, failed: &BTreeSet<NodeName>) {
        let successful = self
            .outcome_writes
            .keys()
            .filter(|name| !failed.contains(*name))
            .cloned()
            .collect::<Vec<_>>();
        for name in successful {
            if let Some(writes) = self.outcome_writes.remove(&name) {
                self.definite_writes.extend(writes);
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Fold {
    executions: u64,
    concurrency: u64,
    loop_entries: u64,
}

struct Analyzer<'a> {
    graph: &'a GraphSpec,
    nodes: BTreeMap<NodeName, NodeInfo<'a>>,
    authored: Vec<NodeName>,
    dependencies: BTreeMap<NodeName, BTreeSet<NodeName>>,
    loops: Vec<NodeName>,
    attempts: BTreeMap<NodeName, PositiveInteger>,
    diagnostics: Vec<GraphDiagnostic>,
    guard_nodes: u64,
    exhaustive_choices: BTreeSet<NodeName>,
    choice_reachability: BTreeMap<NodeName, ChoiceReachability>,
    node_fallthrough: BTreeMap<usize, bool>,
}

impl<'a> Analyzer<'a> {
    fn new(graph: &'a GraphSpec) -> Self {
        Self {
            graph,
            nodes: BTreeMap::new(),
            authored: Vec::new(),
            dependencies: BTreeMap::new(),
            loops: Vec::new(),
            attempts: BTreeMap::new(),
            diagnostics: Vec::new(),
            guard_nodes: 0,
            exhaustive_choices: BTreeSet::new(),
            choice_reachability: BTreeMap::new(),
            node_fallthrough: BTreeMap::new(),
        }
    }

    fn analyze(&mut self) -> Option<StructuralBounds> {
        self.validate_profile();
        let root_path = vec![field_segment("root"), node_segment(self.graph.root.name())];
        self.index_node(&self.graph.root, root_path, 1);
        self.validate_global_limits();

        let initial = Flow {
            defined: required_paths_with_types(&self.graph.initial_input),
            ..Flow::default()
        };
        self.validate_node(&self.graph.root, &initial, &self.graph.initial_input, None);
        self.validate_terminal_coverage();
        let order = self.reference_topological_order();
        let fold = self.fold_node(&self.graph.root);

        if !self.diagnostics.is_empty() {
            return None;
        }
        self.build_bounds(fold?, order)
    }

    fn validate_profile(&mut self) {
        if self.graph.profile != GraphProfile::Full {
            self.emit(
                GraphDiagnosticCode::InvalidGraphShape,
                "production full-v1 verifier accepts only openengine.graph.full/v1",
                vec![field_segment("profile")],
                Vec::new(),
            );
        }
    }

    fn validate_global_limits(&mut self) {
        if self.authored.len() as u64 > FULL_V1_MAX_GRAPH_NODES {
            self.emit(
                GraphDiagnosticCode::CeilingExceeded,
                format!("graph node count exceeds {FULL_V1_MAX_GRAPH_NODES}"),
                vec![field_segment("root")],
                Vec::new(),
            );
        }
        if self.attempts.is_empty() {
            self.emit(
                GraphDiagnosticCode::InvalidGraphShape,
                "full-v1 graph must contain at least one step or verifier",
                vec![field_segment("root")],
                Vec::new(),
            );
        }
        if self.guard_nodes > FULL_V1_MAX_GUARD_NODES {
            self.emit(
                GraphDiagnosticCode::CeilingExceeded,
                format!("graph guard-node count exceeds {FULL_V1_MAX_GUARD_NODES}"),
                vec![field_segment("root")],
                Vec::new(),
            );
        }
    }

    fn build_bounds(&self, fold: Fold, order: Vec<NodeName>) -> Option<StructuralBounds> {
        let max_node_executions = PositiveInteger::new(fold.executions).ok()?;
        let peak_concurrency = PositiveInteger::new(fold.concurrency).ok()?;
        let termination = if self.loops.is_empty() {
            TerminationWitness::Acyclic {
                order: NonEmptyVec::new(order).ok()?,
            }
        } else {
            let ranking = self
                .loops
                .iter()
                .map(|name| FieldPath::new(vec![field_name(name.as_str())]).ok())
                .collect::<Option<Vec<_>>>()?;
            TerminationWitness::Bounded {
                ranking: NonEmptyVec::new(ranking).ok()?,
                max_iterations: PositiveInteger::new(fold.loop_entries).ok()?,
            }
        };
        Some(StructuralBounds {
            termination,
            max_node_executions,
            peak_concurrency,
            attempts_per_node: self.attempts.clone(),
        })
    }

    fn index_node(&mut self, node: &'a GraphNode, path: Vec<DiagnosticPathSegment>, depth: u64) {
        self.index_identity(node, &path);
        self.validate_node_depth(node.name(), &path, depth);
        self.index_children(node, &path, depth);
    }

    fn index_identity(&mut self, node: &'a GraphNode, path: &[DiagnosticPathSegment]) {
        let name = node.name().clone();
        let ordinal = self.authored.len();
        self.authored.push(name.clone());
        self.dependencies.entry(name.clone()).or_default();
        if let Some(previous) = self.nodes.get(&name) {
            self.emit(
                GraphDiagnosticCode::InvalidGraphShape,
                format!("duplicate node name {name}"),
                path.to_vec(),
                vec![previous.node.name().clone()],
            );
        } else {
            self.nodes.insert(
                name.clone(),
                NodeInfo {
                    node,
                    path: path.to_vec(),
                    ordinal,
                },
            );
        }
    }

    fn validate_node_depth(&mut self, name: &NodeName, path: &[DiagnosticPathSegment], depth: u64) {
        if depth > FULL_V1_MAX_GRAPH_DEPTH {
            self.emit(
                GraphDiagnosticCode::CeilingExceeded,
                format!("graph depth exceeds {FULL_V1_MAX_GRAPH_DEPTH}"),
                path.to_vec(),
                vec![name.clone()],
            );
        }
    }

    fn index_children(&mut self, node: &'a GraphNode, path: &[DiagnosticPathSegment], depth: u64) {
        match node {
            GraphNode::Step(step) => {
                self.index_executable(&step.name, step.attempts, path, &step.write_bindings, &[])
            }
            GraphNode::Verifier(verifier) => self.index_executable(
                &verifier.name,
                verifier.attempts,
                path,
                &verifier.write_bindings,
                &[],
            ),
            GraphNode::Seq(group) => self.index_sequence(group, path, depth),
            GraphNode::Choice(group) => self.index_choice(group, path, depth),
            GraphNode::Par(group) => self.index_parallel(group, path, depth),
            GraphNode::Loop(group) => self.index_loop(group, path, depth),
            GraphNode::Map(group) => self.index_node(
                &group.body,
                named_child_path(path, "body", group.body.name()),
                depth + 1,
            ),
            GraphNode::Succeed(_) | GraphNode::Fail(_) => {}
        }
    }

    fn index_sequence(&mut self, group: &'a SeqNode, path: &[DiagnosticPathSegment], depth: u64) {
        for (index, child) in group.children.as_slice().iter().enumerate() {
            self.index_node(
                child,
                child_path(path, "children", index, child.name()),
                depth + 1,
            );
        }
    }

    fn index_choice(&mut self, group: &'a ChoiceNode, path: &[DiagnosticPathSegment], depth: u64) {
        for (index, branch) in group.branches.as_slice().iter().enumerate() {
            self.count_guard(&branch.when, guard_path(path, "branches", index, "when"));
            let branch_path = choice_branch_node_path(path, index, branch.node.name());
            self.index_node(&branch.node, branch_path, depth + 1);
        }
        if let Some(otherwise) = &group.otherwise {
            self.index_node(
                otherwise,
                named_child_path(path, "otherwise", otherwise.name()),
                depth + 1,
            );
        }
    }

    fn index_parallel(&mut self, group: &'a ParNode, path: &[DiagnosticPathSegment], depth: u64) {
        if let Join::First { when } = &group.join {
            self.count_guard(when, field_path(path, &["join", "when"]));
        }
        for (index, branch) in group.branches.as_slice().iter().enumerate() {
            self.index_node(
                branch,
                child_path(path, "branches", index, branch.name()),
                depth + 1,
            );
        }
    }

    fn index_loop(&mut self, group: &'a LoopNode, path: &[DiagnosticPathSegment], depth: u64) {
        self.loops.push(group.name.clone());
        self.count_guard(&group.until, with_field(path, "until"));
        self.index_node(
            &group.body,
            named_child_path(path, "body", group.body.name()),
            depth + 1,
        );
    }

    fn index_executable(
        &mut self,
        name: &NodeName,
        attempts: PositiveInteger,
        path: &[DiagnosticPathSegment],
        bindings: &[WriteBinding],
        extra_dependencies: &[NodeName],
    ) {
        if attempts.get() > FULL_V1_MAX_ATTEMPTS_PER_NODE {
            self.emit(
                GraphDiagnosticCode::CeilingExceeded,
                format!("attempts exceeds full-v1 limit {FULL_V1_MAX_ATTEMPTS_PER_NODE}"),
                with_field(path, "attempts"),
                vec![name.clone()],
            );
        }
        self.attempts.insert(name.clone(), attempts);
        for binding in bindings {
            if binding.value.node != *name {
                self.dependencies
                    .entry(name.clone())
                    .or_default()
                    .insert(binding.value.node.clone());
            }
        }
        self.dependencies
            .entry(name.clone())
            .or_default()
            .extend(extra_dependencies.iter().cloned());
    }

    fn count_guard(&mut self, guard: &Guard, path: Vec<DiagnosticPathSegment>) {
        let count = guard_node_count(guard);
        self.guard_nodes = self.guard_nodes.saturating_add(count);
        for selector in guard_selectors(guard) {
            if selector.name != *self.graph.root.name() {
                // The exact owning-node edge is added during semantic validation.
                self.dependencies.entry(selector.name.clone()).or_default();
            }
        }
        if count > FULL_V1_MAX_GUARD_NODES {
            self.emit(
                GraphDiagnosticCode::CeilingExceeded,
                format!("guard contains more than {FULL_V1_MAX_GUARD_NODES} nodes"),
                path,
                Vec::new(),
            );
        }
    }

    fn validate_node(
        &mut self,
        node: &GraphNode,
        incoming: &Flow,
        state: &PayloadType,
        item: Option<&PayloadType>,
    ) -> Effects {
        let path = self.node_path(node.name());
        self.validate_group_state(node, incoming, state, &path);
        let effects = match node {
            GraphNode::Step(step) => self.validate_step(step, incoming, state, item, &path),
            GraphNode::Verifier(verifier) => {
                self.validate_verifier(verifier, incoming, state, item, &path)
            }
            GraphNode::Seq(group) => self.validate_seq(group, incoming, state, item, &path),
            GraphNode::Choice(group) => {
                self.validate_choice_node(group, incoming, state, item, &path)
            }
            GraphNode::Par(group) => self.validate_par(group, incoming, state, item, &path),
            GraphNode::Loop(group) => self.validate_loop(group, incoming, state, item, &path),
            GraphNode::Map(group) => self.validate_map(group, incoming, state, item, &path),
            GraphNode::Succeed(terminal) => {
                self.validate_succeed(terminal, incoming, state, item, &path)
            }
            GraphNode::Fail(terminal) => terminal_effects(&terminal.name, incoming),
        };
        self.node_fallthrough
            .insert(node_identity(node), effects.falls_through);
        effects
    }

    fn validate_step(
        &mut self,
        step: &StepNode,
        incoming: &Flow,
        state: &PayloadType,
        item: Option<&PayloadType>,
        path: &[DiagnosticPathSegment],
    ) -> Effects {
        self.validate_executable(
            &step.name,
            &step.input,
            &step.output,
            &step.input_bindings,
            &step.write_bindings,
            incoming,
            state,
            item,
            path,
        )
    }

    fn validate_verifier(
        &mut self,
        verifier: &VerifierNode,
        incoming: &Flow,
        state: &PayloadType,
        item: Option<&PayloadType>,
        path: &[DiagnosticPathSegment],
    ) -> Effects {
        self.validate_executable(
            &verifier.name,
            &verifier.input,
            &verifier.output,
            &verifier.input_bindings,
            &verifier.write_bindings,
            incoming,
            state,
            item,
            path,
        )
    }

    fn validate_succeed(
        &mut self,
        terminal: &openengine_cluster_protocol::SucceedNode,
        incoming: &Flow,
        state: &PayloadType,
        item: Option<&PayloadType>,
        path: &[DiagnosticPathSegment],
    ) -> Effects {
        self.validate_input_bindings(
            &terminal.bindings,
            &terminal.output,
            incoming,
            state,
            item,
            path,
            "bindings",
        );
        terminal_effects(&terminal.name, incoming)
    }

    fn validate_group_state(
        &mut self,
        node: &GraphNode,
        flow: &Flow,
        incoming: &PayloadType,
        path: &[DiagnosticPathSegment],
    ) {
        if node_state(node)
            .is_some_and(|state| !is_subtype_with_definitions(incoming, state, &flow.defined))
        {
            self.emit(
                GraphDiagnosticCode::SchemaSafety,
                "incoming state is not a subtype of the group's declared state",
                with_field(path, "state"),
                vec![node.name().clone()],
            );
        }
    }

    fn validate_seq(
        &mut self,
        group: &SeqNode,
        incoming: &Flow,
        enclosing_state: &PayloadType,
        item: Option<&PayloadType>,
        path: &[DiagnosticPathSegment],
    ) -> Effects {
        let mut flow = incoming.clone();
        let mut effects = Effects::default();
        let mut falls_through = true;
        let mut completion = CompletionPredicate::Always;
        for child in group.children.as_slice() {
            let child_effects = self.validate_node(child, &flow, &group.state, item);
            if falls_through {
                flow.apply_effects(&child_effects);
            }
            completion = CompletionPredicate::all([completion, child_effects.completion.clone()]);
            falls_through = falls_through
                && child_effects.falls_through
                && self
                    .completion_is_satisfiable(&completion, path)
                    .unwrap_or(true);
            effects
                .definite_nodes
                .extend(child_effects.definite_nodes.clone());
            effects
                .definite_writes
                .extend(child_effects.definite_writes.clone());
            effects
                .possible_writes
                .extend(child_effects.possible_writes);
            merge_possible_write_types(
                &mut effects.possible_write_types,
                &child_effects.possible_write_types,
            );
            merge_outcome_writes(&mut effects.outcome_writes, &child_effects.outcome_writes);
            if falls_through {
                effects.resolve_successful_writes(&flow.failed);
            }
        }
        effects.exit_failed = flow.failed;
        effects.falls_through = falls_through;
        effects.completion = completion;
        effects.definite_nodes.insert(group.name.clone());
        self.restrict_promotions(
            &group.state,
            enclosing_state,
            &group.promoted_state_paths,
            &mut effects,
            path,
            PromotionRule::Definite,
        );
        effects
    }

    fn validate_choice_node(
        &mut self,
        group: &ChoiceNode,
        incoming: &Flow,
        enclosing_state: &PayloadType,
        item: Option<&PayloadType>,
        path: &[DiagnosticPathSegment],
    ) -> Effects {
        let control = self.validate_choice(group, incoming, path);
        self.choice_reachability
            .insert(group.name.clone(), control.reachability());
        let mut alternatives = Vec::new();
        for (index, branch) in group.branches.as_slice().iter().enumerate() {
            if !control.branch_reachable[index] {
                continue;
            }
            let mut branch_flow = incoming.clone();
            control.branches[index].apply(&mut branch_flow);
            let mut effects = self.validate_node(&branch.node, &branch_flow, &group.state, item);
            self.restrict_completion(&mut effects, control.branch_completion[index].clone(), path);
            alternatives.push(effects);
        }
        match &group.otherwise {
            Some(otherwise) if control.otherwise_reachable => {
                let mut otherwise_flow = incoming.clone();
                control.otherwise.apply(&mut otherwise_flow);
                let mut effects =
                    self.validate_node(otherwise, &otherwise_flow, &group.state, item);
                self.restrict_completion(&mut effects, control.otherwise_completion.clone(), path);
                alternatives.push(effects);
            }
            _ if !control.exhaustive => {
                alternatives.push(Effects {
                    exit_failed: incoming.failed.clone(),
                    falls_through: true,
                    completion: control.otherwise_completion.clone(),
                    ..Effects::default()
                });
            }
            _ => {}
        }
        let mut effects = merge_alternatives(&alternatives);
        effects.definite_nodes.insert(group.name.clone());
        self.restrict_promotions(
            &group.state,
            enclosing_state,
            &group.promoted_state_paths,
            &mut effects,
            path,
            PromotionRule::EveryAlternative,
        );
        effects
    }

    fn validate_par(
        &mut self,
        group: &ParNode,
        incoming: &Flow,
        enclosing_state: &PayloadType,
        item: Option<&PayloadType>,
        path: &[DiagnosticPathSegment],
    ) -> Effects {
        self.validate_join(group, incoming, path);
        let branches = group
            .branches
            .as_slice()
            .iter()
            .map(|branch| self.validate_node(branch, incoming, &group.state, item))
            .collect::<Vec<_>>();
        self.validate_parallel_writes(group, &branches, path);
        let mut effects = self.merge_parallel(&group.join, &branches, path);
        effects.definite_nodes.insert(group.name.clone());
        let rule = if matches!(group.join, Join::All {}) {
            PromotionRule::Definite
        } else {
            PromotionRule::EveryAlternative
        };
        self.restrict_promotions(
            &group.state,
            enclosing_state,
            &group.promoted_state_paths,
            &mut effects,
            path,
            rule,
        );
        effects
    }

    fn merge_parallel(
        &mut self,
        join: &Join,
        branches: &[Effects],
        path: &[DiagnosticPathSegment],
    ) -> Effects {
        let required = parallel_required_completions(join, branches.len());
        if required > branches.len() as u64 {
            // Shape validation owns the invalid quorum diagnostic. Assume continuation so
            // terminal analysis does not cascade it into a misleading reachability error.
            return Effects {
                falls_through: true,
                completion: CompletionPredicate::Always,
                ..merge_parallel_scenarios(branches, required, &[])
            };
        }

        let predicates = branches
            .iter()
            .map(|branch| &branch.completion)
            .collect::<Vec<_>>();
        let Some(assignments) =
            self.assignments_for_completion_predicates(&predicates, &with_field(path, "join"))
        else {
            // The assignment ceiling diagnostic is sufficient. Keep later passes conservative.
            return Effects {
                falls_through: true,
                completion: CompletionPredicate::Always,
                ..merge_parallel_scenarios(branches, required, &[])
            };
        };
        let scenarios = assignments
            .iter()
            .filter_map(|assignment| {
                let completing = branches
                    .iter()
                    .enumerate()
                    .filter_map(|(index, branch)| {
                        branch.completion.evaluate(assignment).then_some(index)
                    })
                    .collect::<BTreeSet<_>>();
                (completing.len() as u64 >= required).then_some(completing)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let mut merged = merge_parallel_scenarios(branches, required, &scenarios);
        merged.completion = CompletionPredicate::at_least(
            required,
            branches
                .iter()
                .map(|branch| branch.completion.clone())
                .collect(),
        );
        merged
    }

    fn validate_join(&mut self, group: &ParNode, incoming: &Flow, path: &[DiagnosticPathSegment]) {
        if let Join::First { when } = &group.join {
            let mut available = incoming.available.clone();
            available.extend(collect_descendant_names(&group.branches));
            for selector in guard_selectors(when) {
                self.dependencies
                    .entry(group.name.clone())
                    .or_default()
                    .insert(selector.name.clone());
            }
            self.validate_guard(
                when,
                &available,
                &field_path(path, &["join", "when"]),
                GraphDiagnosticCode::InvalidGraphShape,
            );
        }
        if let Join::Quorum { count } = group.join {
            if count.get() > group.branches.as_slice().len() as u64 {
                self.emit(
                    GraphDiagnosticCode::InvalidGraphShape,
                    "parallel quorum exceeds branch count",
                    field_path(path, &["join", "count"]),
                    vec![group.name.clone()],
                );
            }
        }
    }

    fn validate_loop(
        &mut self,
        group: &LoopNode,
        incoming: &Flow,
        enclosing_state: &PayloadType,
        item: Option<&PayloadType>,
        path: &[DiagnosticPathSegment],
    ) -> Effects {
        if group.max_iterations.get() > FULL_V1_MAX_LOOP_ITERATIONS {
            self.emit(
                GraphDiagnosticCode::CeilingExceeded,
                format!("maxIterations exceeds full-v1 limit {FULL_V1_MAX_LOOP_ITERATIONS}"),
                with_field(path, "maxIterations"),
                vec![group.name.clone()],
            );
        }
        let mut effects = self.validate_node(&group.body, incoming, &group.state, item);
        self.validate_loop_exit(group, incoming, &effects, path);
        effects.definite_nodes.insert(group.name.clone());
        self.restrict_promotions(
            &group.state,
            enclosing_state,
            &group.promoted_state_paths,
            &mut effects,
            path,
            PromotionRule::Definite,
        );
        effects
    }

    fn validate_loop_exit(
        &mut self,
        group: &LoopNode,
        incoming: &Flow,
        body: &Effects,
        path: &[DiagnosticPathSegment],
    ) {
        let mut available = incoming.available.clone();
        available.extend(body.definite_nodes.clone());
        let until_path = with_field(path, "until");
        let valid = self.validate_guard(
            &group.until,
            &available,
            &until_path,
            GraphDiagnosticCode::LoopExitSatisfiability,
        );
        for selector in guard_selectors(&group.until) {
            let guaranteed = body.definite_nodes.contains(&selector.name)
                && self
                    .nodes
                    .get(&selector.name)
                    .is_some_and(|info| matches!(info.node, GraphNode::Verifier(_)));
            if !guaranteed {
                self.emit(
                    GraphDiagnosticCode::LoopExitSatisfiability,
                    format!(
                        "loop exit selector {} is not a verifier guaranteed in every iteration",
                        selector.name
                    ),
                    until_path.clone(),
                    vec![selector.name.clone(), group.name.clone()],
                );
            }
        }
        if valid && !self.guard_has_satisfying_assignment(&group.until, &until_path) {
            self.emit(
                GraphDiagnosticCode::LoopExitSatisfiability,
                "loop exit guard is unsatisfiable",
                until_path,
                vec![group.name.clone()],
            );
        }
    }

    fn validate_map(
        &mut self,
        group: &MapNode,
        incoming: &Flow,
        enclosing_state: &PayloadType,
        item: Option<&PayloadType>,
        path: &[DiagnosticPathSegment],
    ) -> Effects {
        if group.max_items.get() > FULL_V1_MAX_MAP_ITEMS {
            self.emit(
                GraphDiagnosticCode::CeilingExceeded,
                format!("maxItems exceeds full-v1 limit {FULL_V1_MAX_MAP_ITEMS}"),
                with_field(path, "maxItems"),
                vec![group.name.clone()],
            );
        }
        let item_type = self.validate_map_selector(group, incoming, item, path);
        let body = self.validate_node(&group.body, incoming, &group.state, item_type.as_ref());
        let mut effects = Effects {
            definite_nodes: BTreeSet::from([group.name.clone()]),
            possible_writes: body.possible_writes.clone(),
            possible_write_types: body.possible_write_types.clone(),
            exit_failed: incoming.failed.union(&body.exit_failed).cloned().collect(),
            falls_through: true,
            completion: CompletionPredicate::Always,
            ..Effects::default()
        };
        for promoted in &group.promoted_state_paths {
            if is_required_path(&group.state, promoted)
                && path_type(&group.state, promoted).is_some()
            {
                if let Some(write) = body.possible_writes.get(promoted) {
                    effects
                        .definite_writes
                        .insert(promoted.clone(), write.clone());
                }
            }
        }
        self.restrict_promotions(
            &group.state,
            enclosing_state,
            &group.promoted_state_paths,
            &mut effects,
            path,
            PromotionRule::Map,
        );
        effects
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_executable(
        &mut self,
        name: &NodeName,
        input: &PayloadType,
        output: &PayloadType,
        input_bindings: &[InputBinding],
        write_bindings: &[WriteBinding],
        incoming: &Flow,
        state: &PayloadType,
        item: Option<&PayloadType>,
        path: &[DiagnosticPathSegment],
    ) -> Effects {
        self.validate_input_bindings(
            input_bindings,
            input,
            incoming,
            state,
            item,
            path,
            "inputBindings",
        );
        let writes =
            self.validate_write_bindings(name, output, write_bindings, incoming, state, path);
        let outcome_writes = if writes.is_empty() {
            BTreeMap::new()
        } else {
            BTreeMap::from([(name.clone(), writes.clone())])
        };
        let possible_write_types = writes
            .iter()
            .map(|(path, write)| (path.clone(), vec![write.value_type.clone()]))
            .collect();
        Effects {
            definite_nodes: BTreeSet::from([name.clone()]),
            possible_writes: writes,
            possible_write_types,
            outcome_writes,
            exit_failed: incoming
                .failed
                .union(&BTreeSet::from([name.clone()]))
                .cloned()
                .collect(),
            falls_through: true,
            completion: CompletionPredicate::Always,
            ..Effects::default()
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_write_bindings(
        &mut self,
        name: &NodeName,
        output: &PayloadType,
        bindings: &[WriteBinding],
        incoming: &Flow,
        state: &PayloadType,
        path: &[DiagnosticPathSegment],
    ) -> Writes {
        let mut writes = Writes::new();
        let mut targets = Vec::<FieldPath>::new();
        for (index, binding) in bindings.iter().enumerate() {
            let binding_path = indexed_field_path(path, "writeBindings", index);
            if let Some(producer) =
                self.validate_write_binding(name, output, binding, incoming, state, &binding_path)
            {
                writes.insert(binding.target.clone(), producer);
            }
            self.validate_target_overlap(
                &targets,
                &binding.target,
                &field_path(&binding_path, &["target"]),
                "executable has overlapping write targets",
                vec![name.clone()],
            );
            targets.push(binding.target.clone());
        }
        writes
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_write_binding(
        &mut self,
        name: &NodeName,
        output: &PayloadType,
        binding: &WriteBinding,
        incoming: &Flow,
        state: &PayloadType,
        path: &[DiagnosticPathSegment],
    ) -> Option<WriteFact> {
        let target = path_type(state, &binding.target);
        if target.is_none() {
            self.emit(
                GraphDiagnosticCode::SchemaSafety,
                "write target does not exist in node state",
                with_field(path, "target"),
                vec![name.clone()],
            );
        }
        let producer = self.validate_output_selector(&binding.value, name, incoming, output, path);
        if let (Some(producer), Some(target)) = (&producer, target) {
            if !producer.value_type.is_subtype_of(target) {
                self.emit(
                    GraphDiagnosticCode::SchemaSafety,
                    "write value is not a subtype of its state target",
                    path.to_vec(),
                    vec![binding.value.node.clone(), name.clone()],
                );
            }
        }
        producer
            .map(|producer| WriteFact {
                guaranteed_paths: guaranteed_write_paths(
                    &binding.target,
                    &producer.value_type,
                    producer.definitely_present,
                ),
                value_type: producer.value_type,
            })
            .filter(|_| target.is_some())
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_input_bindings(
        &mut self,
        bindings: &[InputBinding],
        target_payload: &PayloadType,
        incoming: &Flow,
        state: &PayloadType,
        item: Option<&PayloadType>,
        node_path: &[DiagnosticPathSegment],
        field: &str,
    ) {
        let mut targets = Vec::<FieldPath>::new();
        for (index, binding) in bindings.iter().enumerate() {
            let binding_path = indexed_field_path(node_path, field, index);
            self.validate_input_binding(
                binding,
                target_payload,
                incoming,
                state,
                item,
                &binding_path,
            );
            self.validate_target_overlap(
                &targets,
                &binding.target,
                &with_field(&binding_path, "target"),
                "bindings have overlapping targets",
                Vec::new(),
            );
            targets.push(binding.target.clone());
        }
        self.validate_required_binding_targets(target_payload, &targets, node_path, field);
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_input_binding(
        &mut self,
        binding: &InputBinding,
        target_payload: &PayloadType,
        incoming: &Flow,
        state: &PayloadType,
        item: Option<&PayloadType>,
        path: &[DiagnosticPathSegment],
    ) {
        let target = path_type(target_payload, &binding.target);
        if target.is_none() {
            self.emit(
                GraphDiagnosticCode::SchemaSafety,
                "binding target does not exist in declared payload",
                with_field(path, "target"),
                Vec::new(),
            );
        }
        let source = self.validate_data_selector(
            &binding.value,
            incoming,
            state,
            item,
            &with_field(path, "value"),
        );
        if let (Some(source), Some(target)) = (source, target) {
            if !source.is_subtype_of(target) {
                self.emit(
                    GraphDiagnosticCode::SchemaSafety,
                    "binding source is not a subtype of its target",
                    path.to_vec(),
                    Vec::new(),
                );
            }
        }
    }

    fn validate_required_binding_targets(
        &mut self,
        target_payload: &PayloadType,
        targets: &[FieldPath],
        node_path: &[DiagnosticPathSegment],
        field: &str,
    ) {
        for required in required_leaf_paths(target_payload) {
            if !targets
                .iter()
                .any(|target| path_is_prefix(target, &required))
            {
                self.emit(
                    GraphDiagnosticCode::UndefinedRead,
                    format!(
                        "required payload target {} is not defined by a binding",
                        display_field_path(&required)
                    ),
                    with_field(node_path, field),
                    Vec::new(),
                );
            }
        }
    }

    fn validate_target_overlap(
        &mut self,
        previous: &[FieldPath],
        target: &FieldPath,
        path: &[DiagnosticPathSegment],
        message: &str,
        related_nodes: Vec<NodeName>,
    ) {
        if previous.iter().any(|other| paths_overlap(other, target)) {
            self.emit(
                GraphDiagnosticCode::WriteConflict,
                message,
                path.to_vec(),
                related_nodes,
            );
        }
    }

    fn validate_data_selector<'b>(
        &mut self,
        selector: &DataSelector,
        incoming: &Flow,
        state: &'b PayloadType,
        item: Option<&'b PayloadType>,
        path: &[DiagnosticPathSegment],
    ) -> Option<PayloadType> {
        let (payload, selected, defined) = match selector {
            DataSelector::State { path: selected } => (
                Some(state),
                selected,
                is_required_path(state, selected) || is_defined(&incoming.defined, selected),
            ),
            DataSelector::Item { path: selected } => (
                item,
                selected,
                item.is_some_and(|payload| is_required_path(payload, selected)),
            ),
        };
        let Some(payload) = payload else {
            self.emit(
                GraphDiagnosticCode::InvalidGraphShape,
                "item selector is legal only inside a map body",
                path.to_vec(),
                Vec::new(),
            );
            return None;
        };
        let selected_type = path_type(payload, selected);
        if selected_type.is_none() {
            self.emit(
                GraphDiagnosticCode::SchemaSafety,
                "selector path does not exist in its payload type",
                path.to_vec(),
                Vec::new(),
            );
        } else if !defined {
            self.emit(
                GraphDiagnosticCode::UndefinedRead,
                "selector path is not definitely defined on every reaching path",
                path.to_vec(),
                Vec::new(),
            );
        }
        match selector {
            DataSelector::State { .. } => incoming
                .defined
                .get(selected)
                .cloned()
                .or_else(|| selected_type.cloned()),
            DataSelector::Item { .. } => selected_type.cloned(),
        }
    }

    fn validate_output_selector(
        &mut self,
        selector: &NodeOutputSelector,
        current: &NodeName,
        incoming: &Flow,
        current_output: &PayloadType,
        path: &[DiagnosticPathSegment],
    ) -> Option<SelectedOutput> {
        self.validate_output_availability(selector, current, incoming, path);
        let selected = self.output_selector(selector, current, current_output);
        if selected.is_none() {
            self.emit(
                GraphDiagnosticCode::SchemaSafety,
                "node output selector channel or path is invalid",
                with_field(path, "value"),
                vec![selector.node.clone()],
            );
        }
        selected
    }

    fn validate_output_availability(
        &mut self,
        selector: &NodeOutputSelector,
        current: &NodeName,
        incoming: &Flow,
        path: &[DiagnosticPathSegment],
    ) {
        if selector.node != *current && !incoming.available.contains(&selector.node) {
            self.emit(
                GraphDiagnosticCode::UndefinedRead,
                format!("node output {} does not dominate this read", selector.node),
                field_path(path, &["value"]),
                vec![selector.node.clone(), current.clone()],
            );
        }
        if incoming.failed.contains(&selector.node)
            && matches!(
                selector.channel,
                NodeOutputChannel::Out | NodeOutputChannel::Signal | NodeOutputChannel::Diagnostic
            )
        {
            self.emit(
                GraphDiagnosticCode::UndefinedRead,
                format!(
                    "node output {} is unavailable on its error path",
                    selector.node
                ),
                field_path(path, &["value"]),
                vec![selector.node.clone(), current.clone()],
            );
        }
    }

    fn output_selector(
        &self,
        selector: &NodeOutputSelector,
        current: &NodeName,
        current_output: &PayloadType,
    ) -> Option<SelectedOutput> {
        match selector.channel {
            NodeOutputChannel::Out if selector.node == *current => {
                selected_output_path(current_output, &selector.path)
            }
            NodeOutputChannel::Out => self
                .nodes
                .get(&selector.node)
                .map(|info| info.node)
                .and_then(executable_output)
                .and_then(|payload| selected_output_path(payload, &selector.path)),
            NodeOutputChannel::Signal => {
                self.selector_verifier(&selector.node).and_then(|verifier| {
                    signal_path_type(verifier, &selector.path).map(|value_type| SelectedOutput {
                        value_type,
                        definitely_present: true,
                    })
                })
            }
            NodeOutputChannel::Diagnostic => self
                .selector_verifier(&selector.node)
                .and_then(|verifier| selected_output_path(&verifier.diagnostic, &selector.path)),
        }
    }

    fn selector_verifier(&self, name: &NodeName) -> Option<&VerifierNode> {
        match self.nodes.get(name).map(|info| info.node) {
            Some(GraphNode::Verifier(verifier)) => Some(verifier),
            _ => None,
        }
    }

    fn validate_map_selector(
        &mut self,
        map: &MapNode,
        incoming: &Flow,
        item: Option<&PayloadType>,
        path: &[DiagnosticPathSegment],
    ) -> Option<PayloadType> {
        let selected = self.validate_data_selector(
            &map.over,
            incoming,
            &map.state,
            item,
            &with_field(path, "over"),
        );
        match selected {
            Some(PayloadType::Array { items }) => Some((*items).clone()),
            Some(_) => {
                self.emit(
                    GraphDiagnosticCode::SchemaSafety,
                    "map selector must resolve to an array",
                    with_field(path, "over"),
                    vec![map.name.clone()],
                );
                None
            }
            None => None,
        }
    }

    fn validate_choice(
        &mut self,
        choice: &ChoiceNode,
        incoming: &Flow,
        path: &[DiagnosticPathSegment],
    ) -> ChoiceControl {
        if !self.validate_choice_guards(choice, incoming, path) {
            return ChoiceControl::unknown(choice);
        }
        let guards = choice
            .branches
            .as_slice()
            .iter()
            .map(|branch| &branch.when)
            .collect::<Vec<_>>();
        let Some(assignments) = self.assignments_for_guards(&guards, &with_field(path, "branches"))
        else {
            return ChoiceControl::unknown(choice);
        };
        let (branches, branch_reachable, covered) =
            self.choice_branch_outcomes(choice, &assignments, path);
        let uncovered = assignments
            .iter()
            .zip(covered)
            .filter_map(|(assignment, covered)| (!covered).then_some(assignment))
            .collect::<Vec<_>>();
        let (exhaustive, otherwise_reachable) =
            self.record_choice_exhaustiveness(choice, &uncovered, path);
        ChoiceControl {
            branches,
            branch_reachable,
            branch_completion: choice_branch_completion_predicates(choice),
            otherwise: outcome_refinement(&uncovered),
            otherwise_reachable,
            otherwise_completion: choice_otherwise_completion_predicate(choice),
            exhaustive,
        }
    }

    fn validate_choice_guards(
        &mut self,
        choice: &ChoiceNode,
        incoming: &Flow,
        path: &[DiagnosticPathSegment],
    ) -> bool {
        let mut valid = true;
        for (index, branch) in choice.branches.as_slice().iter().enumerate() {
            let guard_path = guard_path(path, "branches", index, "when");
            valid &= self.validate_guard(
                &branch.when,
                &incoming.available,
                &guard_path,
                GraphDiagnosticCode::ChoiceExhaustiveness,
            );
            for selector in guard_selectors(&branch.when) {
                self.dependencies
                    .entry(choice.name.clone())
                    .or_default()
                    .insert(selector.name.clone());
            }
        }
        valid
    }

    fn choice_branch_outcomes(
        &mut self,
        choice: &ChoiceNode,
        assignments: &[Assignment],
        path: &[DiagnosticPathSegment],
    ) -> (Vec<OutcomeRefinement>, Vec<bool>, Vec<bool>) {
        let mut prior = vec![false; assignments.len()];
        let mut outcomes = Vec::with_capacity(choice.branches.as_slice().len());
        let mut reachable = Vec::with_capacity(choice.branches.as_slice().len());
        for (index, branch) in choice.branches.as_slice().iter().enumerate() {
            let mut residual = Vec::new();
            for (assignment_index, assignment) in assignments.iter().enumerate() {
                let matches = evaluate_guard(&branch.when, assignment);
                if matches && !prior[assignment_index] {
                    residual.push(assignment);
                }
                prior[assignment_index] |= matches;
            }
            if residual.is_empty() {
                self.emit(
                    GraphDiagnosticCode::ChoiceExhaustiveness,
                    "choice branch is unreachable after excluding earlier branches",
                    guard_path(path, "branches", index, "when"),
                    vec![choice.name.clone()],
                );
            }
            reachable.push(!residual.is_empty());
            outcomes.push(outcome_refinement(&residual));
        }
        (outcomes, reachable, prior)
    }

    fn record_choice_exhaustiveness(
        &mut self,
        choice: &ChoiceNode,
        uncovered: &[&Assignment],
        path: &[DiagnosticPathSegment],
    ) -> (bool, bool) {
        let exhaustive = choice.otherwise.is_some() || uncovered.is_empty();
        if choice.otherwise.is_none() && !exhaustive {
            self.emit(
                GraphDiagnosticCode::ChoiceExhaustiveness,
                "choice branches do not cover every legal control assignment",
                with_field(path, "branches"),
                vec![choice.name.clone()],
            );
        }
        let otherwise_reachable = choice.otherwise.is_some() && !uncovered.is_empty();
        if let Some(otherwise) = &choice.otherwise {
            if !otherwise_reachable {
                self.emit(
                    GraphDiagnosticCode::ChoiceExhaustiveness,
                    "choice otherwise is unreachable because earlier branches cover every legal control assignment",
                    named_child_path(path, "otherwise", otherwise.name()),
                    vec![choice.name.clone()],
                );
            }
        }
        if exhaustive {
            self.exhaustive_choices.insert(choice.name.clone());
        }
        (exhaustive, otherwise_reachable)
    }

    fn validate_guard(
        &mut self,
        guard: &Guard,
        available: &BTreeSet<NodeName>,
        path: &[DiagnosticPathSegment],
        code: GraphDiagnosticCode,
    ) -> bool {
        let mut valid = true;
        for (selector, map_aggregate) in guard_selector_uses(guard) {
            let map_available = map_aggregate
                && self
                    .map_owner(&selector.name)
                    .is_some_and(|owner| available.contains(owner));
            if !available.contains(&selector.name) && !map_available {
                self.emit(
                    GraphDiagnosticCode::UndefinedRead,
                    format!(
                        "control selector {} does not dominate this guard",
                        selector.name
                    ),
                    path.to_vec(),
                    vec![selector.name.clone()],
                );
                valid = false;
            }
            if self.selector_domain(selector).is_none() {
                self.emit(
                    code,
                    format!("illegal control selector for node {}", selector.name),
                    path.to_vec(),
                    vec![selector.name.clone()],
                );
                valid = false;
            }
        }
        self.validate_guard_labels(guard, path, code, &mut valid);
        valid
    }

    fn validate_guard_labels(
        &mut self,
        guard: &Guard,
        path: &[DiagnosticPathSegment],
        code: GraphDiagnosticCode,
        valid: &mut bool,
    ) {
        match guard {
            Guard::In { value, labels } => self.validate_selector_labels(
                value,
                labels.values(),
                path,
                code,
                "guard labels exceed the selector's closed domain",
                valid,
            ),
            Guard::All { guards } | Guard::Any { guards } => {
                self.validate_nested_guard_labels(guards.as_slice(), path, code, valid)
            }
            Guard::Not { guard } => {
                self.validate_guard_labels(guard, &with_field(path, "guard"), code, valid)
            }
            Guard::KOfN {
                count,
                values,
                labels,
            } => self.validate_k_of_n_labels(
                count.get(),
                values.as_slice(),
                labels.values(),
                path,
                code,
                valid,
            ),
            Guard::KOfMap {
                count,
                value,
                labels,
            } => self.validate_k_of_map_labels(
                count.get(),
                value,
                labels.values(),
                path,
                code,
                valid,
            ),
        }
    }

    fn validate_nested_guard_labels(
        &mut self,
        guards: &[Guard],
        path: &[DiagnosticPathSegment],
        code: GraphDiagnosticCode,
        valid: &mut bool,
    ) {
        for (index, child) in guards.iter().enumerate() {
            self.validate_guard_labels(
                child,
                &indexed_field_path(path, "guards", index),
                code,
                valid,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_selector_labels(
        &mut self,
        selector: &ControlSelector,
        labels: &[EnumLabel],
        path: &[DiagnosticPathSegment],
        code: GraphDiagnosticCode,
        message: &str,
        valid: &mut bool,
    ) {
        let outside_domain = self
            .selector_domain(selector)
            .is_some_and(|domain| !labels.iter().all(|label| domain.contains(label)));
        if outside_domain {
            self.emit(
                code,
                message,
                with_field(path, "labels"),
                vec![selector.name.clone()],
            );
            *valid = false;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_k_of_n_labels(
        &mut self,
        count: u64,
        selectors: &[ControlSelector],
        labels: &[EnumLabel],
        path: &[DiagnosticPathSegment],
        code: GraphDiagnosticCode,
        valid: &mut bool,
    ) {
        if count > selectors.len() as u64 {
            self.emit(
                code,
                "k_of_n count exceeds selector count",
                with_field(path, "count"),
                Vec::new(),
            );
            *valid = false;
        }
        let union = selectors
            .iter()
            .filter_map(|selector| self.selector_domain(selector))
            .flatten()
            .collect::<BTreeSet<_>>();
        if !labels.iter().all(|label| union.contains(label)) {
            self.emit(
                code,
                "k_of_n labels exceed the selectors' combined closed domains",
                with_field(path, "labels"),
                selectors
                    .iter()
                    .map(|selector| selector.name.clone())
                    .collect(),
            );
            *valid = false;
        }
        self.validate_k_of_n_intersections(selectors, labels, path, code, valid);
    }

    fn validate_k_of_n_intersections(
        &mut self,
        selectors: &[ControlSelector],
        labels: &[EnumLabel],
        path: &[DiagnosticPathSegment],
        code: GraphDiagnosticCode,
        valid: &mut bool,
    ) {
        for selector in selectors {
            let disjoint = self
                .selector_domain(selector)
                .is_some_and(|domain| !labels.iter().any(|label| domain.contains(label)));
            if disjoint {
                self.emit(
                    code,
                    "k_of_n labels do not intersect a selector domain",
                    with_field(path, "labels"),
                    vec![selector.name.clone()],
                );
                *valid = false;
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn validate_k_of_map_labels(
        &mut self,
        count: u64,
        selector: &ControlSelector,
        labels: &[EnumLabel],
        path: &[DiagnosticPathSegment],
        code: GraphDiagnosticCode,
        valid: &mut bool,
    ) {
        if count > self.map_control_cardinality(selector).unwrap_or(1) {
            self.emit(
                code,
                "k_of_map count exceeds the selected map bound",
                with_field(path, "count"),
                vec![selector.name.clone()],
            );
            *valid = false;
        }
        self.validate_selector_labels(
            selector,
            labels,
            path,
            code,
            "k_of_map labels exceed the selector's closed domain",
            valid,
        );
    }

    fn selector_domain(&self, selector: &ControlSelector) -> Option<Vec<EnumLabel>> {
        let node = self.nodes.get(&selector.name)?.node;
        match selector.source {
            ControlSource::Signal => {
                let GraphNode::Verifier(verifier) = node else {
                    return None;
                };
                let field = selector.field.as_ref()?;
                verifier
                    .signals
                    .get(field)
                    .map(|labels| labels.values().to_vec())
            }
            ControlSource::Error => {
                if selector.field.is_some()
                    || !matches!(node, GraphNode::Step(_) | GraphNode::Verifier(_))
                {
                    return None;
                }
                Some(error_labels())
            }
            ControlSource::Group => group_domain(node, selector.field.as_ref()),
        }
    }

    fn map_control_cardinality(&self, selector: &ControlSelector) -> Option<u64> {
        match self.nodes.get(&selector.name)?.node {
            GraphNode::Map(map) => Some(map.max_items.get()),
            _ => self.map_owner(&selector.name).and_then(|owner| {
                self.nodes.get(owner).and_then(|info| match info.node {
                    GraphNode::Map(map) => Some(map.max_items.get()),
                    _ => None,
                })
            }),
        }
    }

    fn map_owner(&self, target: &NodeName) -> Option<&NodeName> {
        find_map_owner(&self.graph.root, target, None)
    }

    fn guard_has_satisfying_assignment(
        &mut self,
        guard: &Guard,
        path: &[DiagnosticPathSegment],
    ) -> bool {
        self.assignments_for_guards(&[guard], path)
            .is_some_and(|assignments| {
                assignments
                    .iter()
                    .any(|assignment| evaluate_guard(guard, assignment))
            })
    }

    fn restrict_completion(
        &mut self,
        effects: &mut Effects,
        condition: CompletionPredicate,
        path: &[DiagnosticPathSegment],
    ) {
        effects.completion = CompletionPredicate::all([condition, effects.completion.clone()]);
        effects.falls_through = effects.falls_through
            && self
                .completion_is_satisfiable(&effects.completion, path)
                .unwrap_or(true);
    }

    fn completion_is_satisfiable(
        &mut self,
        predicate: &CompletionPredicate,
        path: &[DiagnosticPathSegment],
    ) -> Option<bool> {
        self.assignments_for_completion_predicates(&[predicate], path)
            .map(|assignments| {
                assignments
                    .iter()
                    .any(|assignment| predicate.evaluate(assignment))
            })
    }

    fn assignments_for_completion_predicates(
        &mut self,
        predicates: &[&CompletionPredicate],
        path: &[DiagnosticPathSegment],
    ) -> Option<Vec<Assignment>> {
        let mut guards = Vec::new();
        for predicate in predicates {
            predicate.collect_guards(&mut guards);
        }
        self.assignments_for_guards(&guards, path)
    }

    fn assignments_for_guards(
        &mut self,
        guards: &[&Guard],
        path: &[DiagnosticPathSegment],
    ) -> Option<Vec<Assignment>> {
        let (selectors, map_aggregates) = self.assignment_inputs(guards);
        let Some(dimensions) = build_dimensions(&selectors, &self.nodes, &map_aggregates) else {
            self.assignment_ceiling(path);
            return None;
        };
        if !self.assignment_count_is_bounded(&dimensions, path) {
            return None;
        }
        Some(enumerate_assignments(dimensions))
    }

    fn assignment_inputs(
        &self,
        guards: &[&Guard],
    ) -> (Vec<ControlSelector>, BTreeMap<SelectorKey, u64>) {
        let selectors = guards
            .iter()
            .flat_map(|guard| guard_selectors(guard))
            .cloned()
            .collect::<Vec<_>>();
        let aggregates = guards
            .iter()
            .flat_map(|guard| guard_selector_uses(guard))
            .filter_map(|(selector, aggregate)| {
                aggregate
                    .then(|| self.map_control_cardinality(selector))
                    .flatten()
                    .map(|maximum| (SelectorKey::from(selector), maximum))
            })
            .collect();
        (selectors, aggregates)
    }

    fn assignment_count_is_bounded(
        &mut self,
        dimensions: &[Vec<Assignment>],
        path: &[DiagnosticPathSegment],
    ) -> bool {
        let mut total = 1_u64;
        for dimension in dimensions {
            total = match total.checked_mul(dimension.len() as u64) {
                Some(value) if value <= FULL_V1_MAX_GUARD_ASSIGNMENTS => value,
                _ => {
                    self.assignment_ceiling(path);
                    return false;
                }
            };
        }
        true
    }

    fn assignment_ceiling(&mut self, path: &[DiagnosticPathSegment]) {
        self.emit(
            GraphDiagnosticCode::CeilingExceeded,
            format!("control assignment space exceeds {FULL_V1_MAX_GUARD_ASSIGNMENTS}"),
            path.to_vec(),
            Vec::new(),
        );
    }

    fn validate_parallel_writes(
        &mut self,
        par: &ParNode,
        branches: &[Effects],
        path: &[DiagnosticPathSegment],
    ) {
        for (promotion_index, promoted) in par.promoted_state_paths.iter().enumerate() {
            let promoted_types = branches
                .iter()
                .flat_map(|branch| {
                    branch
                        .possible_write_types
                        .get(promoted)
                        .into_iter()
                        .flatten()
                })
                .collect::<Vec<_>>();
            for left in 0..promoted_types.len() {
                for right in (left + 1)..promoted_types.len() {
                    if !compatible_types(promoted_types[left], promoted_types[right]) {
                        self.emit(
                            GraphDiagnosticCode::SchemaSafety,
                            "parallel promotion has incompatible branch value types",
                            indexed_field_path(path, "promotedStatePaths", promotion_index),
                            vec![par.name.clone()],
                        );
                    }
                }
            }
        }
        if !matches!(par.join, Join::All {}) {
            return;
        }
        for left in 0..branches.len() {
            for right in (left + 1)..branches.len() {
                for left_path in branches[left].possible_writes.keys() {
                    for right_path in branches[right].possible_writes.keys() {
                        if paths_overlap(left_path, right_path) {
                            self.emit(
                                GraphDiagnosticCode::WriteConflict,
                                "parallel all branches have overlapping writes",
                                with_field(path, "branches"),
                                vec![
                                    par.branches.as_slice()[left].name().clone(),
                                    par.branches.as_slice()[right].name().clone(),
                                ],
                            );
                        }
                    }
                }
            }
        }
    }

    fn restrict_promotions(
        &mut self,
        group_state: &PayloadType,
        enclosing_state: &PayloadType,
        promoted: &[FieldPath],
        effects: &mut Effects,
        path: &[DiagnosticPathSegment],
        rule: PromotionRule,
    ) {
        let allowed = promoted.iter().cloned().collect::<BTreeSet<_>>();
        for (index, promoted_path) in promoted.iter().enumerate() {
            let diagnostic_path = indexed_field_path(path, "promotedStatePaths", index);
            let promoted_type = path_type(group_state, promoted_path);
            if promoted_type.is_none() {
                self.emit(
                    GraphDiagnosticCode::SchemaSafety,
                    "promoted state path does not exist in group state",
                    diagnostic_path.clone(),
                    Vec::new(),
                );
                continue;
            }
            match path_type(enclosing_state, promoted_path) {
                None => self.emit(
                    GraphDiagnosticCode::SchemaSafety,
                    "promoted state path does not exist in enclosing state",
                    diagnostic_path.clone(),
                    Vec::new(),
                ),
                Some(target)
                    if effects
                        .possible_write_types
                        .get(promoted_path)
                        .is_some_and(|sources| {
                            sources.iter().any(|source| !source.is_subtype_of(target))
                        }) =>
                {
                    self.emit(
                        GraphDiagnosticCode::SchemaSafety,
                        "promoted value is not a subtype of its enclosing state target",
                        diagnostic_path.clone(),
                        Vec::new(),
                    );
                }
                Some(_) => {}
            }
            let defined = is_required_path(group_state, promoted_path)
                || effects
                    .definite_writes
                    .values()
                    .any(|write| write.guaranteed_paths.contains_key(promoted_path));
            if !defined {
                let message = match rule {
                    PromotionRule::EveryAlternative => {
                        "promoted path is not defined by every completing alternative"
                    }
                    PromotionRule::Map => {
                        "map promotion is not defined for the empty-map completion"
                    }
                    PromotionRule::Definite => {
                        "promoted path is not definitely defined on completion"
                    }
                };
                self.emit(
                    GraphDiagnosticCode::UndefinedRead,
                    message,
                    diagnostic_path,
                    Vec::new(),
                );
            }
        }
        retain_promoted_writes(effects, &allowed);
    }

    fn fold_node(&mut self, node: &GraphNode) -> Option<Fold> {
        let path = self.node_path(node.name());
        let fold = match node {
            GraphNode::Step(_) | GraphNode::Verifier(_) => Fold {
                executions: 1,
                concurrency: 1,
                loop_entries: 0,
            },
            GraphNode::Succeed(_) | GraphNode::Fail(_) => Fold::default(),
            GraphNode::Seq(group) => self.fold_seq(group, &path)?,
            GraphNode::Choice(group) => self.fold_choice(group)?,
            GraphNode::Par(group) => self.fold_par(group, &path)?,
            GraphNode::Map(group) => self.fold_map(group, &path)?,
            GraphNode::Loop(group) => self.fold_loop(group, &path)?,
        };
        self.check_fold_limits(fold, &path)
    }

    fn fold_seq(&mut self, group: &SeqNode, path: &[DiagnosticPathSegment]) -> Option<Fold> {
        let children = group
            .children
            .as_slice()
            .iter()
            .filter_map(|child| self.fold_node(child))
            .collect::<Vec<_>>();
        Some(Fold {
            executions: self.checked_sum(
                children.iter().map(|fold| fold.executions),
                FULL_V1_MAX_NODE_EXECUTIONS,
                path,
                "children",
                "node executions",
            )?,
            concurrency: children
                .iter()
                .map(|fold| fold.concurrency)
                .max()
                .unwrap_or(0),
            loop_entries: self.checked_sum(
                children.iter().map(|fold| fold.loop_entries),
                FULL_V1_MAX_LOOP_ENTRIES,
                path,
                "children",
                "loop entries",
            )?,
        })
    }

    fn fold_choice(&mut self, group: &ChoiceNode) -> Option<Fold> {
        let mut children = group
            .branches
            .as_slice()
            .iter()
            .filter_map(|branch| self.fold_node(&branch.node))
            .collect::<Vec<_>>();
        if let Some(otherwise) = &group.otherwise {
            children.push(self.fold_node(otherwise)?);
        }
        Some(Fold {
            executions: children
                .iter()
                .map(|fold| fold.executions)
                .max()
                .unwrap_or(0),
            concurrency: children
                .iter()
                .map(|fold| fold.concurrency)
                .max()
                .unwrap_or(0),
            loop_entries: children
                .iter()
                .map(|fold| fold.loop_entries)
                .max()
                .unwrap_or(0),
        })
    }

    fn fold_par(&mut self, group: &ParNode, path: &[DiagnosticPathSegment]) -> Option<Fold> {
        let children = group
            .branches
            .as_slice()
            .iter()
            .filter_map(|branch| self.fold_node(branch))
            .collect::<Vec<_>>();
        Some(Fold {
            executions: self.checked_sum(
                children.iter().map(|fold| fold.executions),
                FULL_V1_MAX_NODE_EXECUTIONS,
                path,
                "branches",
                "node executions",
            )?,
            concurrency: self.checked_sum(
                children.iter().map(|fold| fold.concurrency),
                FULL_V1_MAX_PEAK_CONCURRENCY,
                path,
                "branches",
                "peak concurrency",
            )?,
            loop_entries: self.checked_sum(
                children.iter().map(|fold| fold.loop_entries),
                FULL_V1_MAX_LOOP_ENTRIES,
                path,
                "branches",
                "loop entries",
            )?,
        })
    }

    fn fold_map(&mut self, group: &MapNode, path: &[DiagnosticPathSegment]) -> Option<Fold> {
        let body = self.fold_node(&group.body)?;
        Some(Fold {
            executions: self.checked_product(
                group.max_items.get(),
                body.executions,
                FULL_V1_MAX_NODE_EXECUTIONS,
                path,
                "maxItems",
                "node executions",
            )?,
            concurrency: self.checked_product(
                group.max_items.get(),
                body.concurrency,
                FULL_V1_MAX_PEAK_CONCURRENCY,
                path,
                "maxItems",
                "peak concurrency",
            )?,
            loop_entries: self.checked_product(
                group.max_items.get(),
                body.loop_entries,
                FULL_V1_MAX_LOOP_ENTRIES,
                path,
                "maxItems",
                "loop entries",
            )?,
        })
    }

    fn fold_loop(&mut self, group: &LoopNode, path: &[DiagnosticPathSegment]) -> Option<Fold> {
        let body = self.fold_node(&group.body)?;
        let entries = body.loop_entries.checked_add(1).or_else(|| {
            self.ceiling(path, "maxIterations", "loop-entry arithmetic overflow");
            None
        })?;
        Some(Fold {
            executions: self.checked_product(
                group.max_iterations.get(),
                body.executions,
                FULL_V1_MAX_NODE_EXECUTIONS,
                path,
                "maxIterations",
                "node executions",
            )?,
            concurrency: body.concurrency,
            loop_entries: self.checked_product(
                group.max_iterations.get(),
                entries,
                FULL_V1_MAX_LOOP_ENTRIES,
                path,
                "maxIterations",
                "loop entries",
            )?,
        })
    }

    fn check_fold_limits(&mut self, fold: Fold, path: &[DiagnosticPathSegment]) -> Option<Fold> {
        if fold.executions > FULL_V1_MAX_NODE_EXECUTIONS {
            self.ceiling(path, "kind", "node executions exceed full-v1 ceiling");
            return None;
        }
        if fold.concurrency > FULL_V1_MAX_PEAK_CONCURRENCY {
            self.ceiling(path, "kind", "peak concurrency exceeds full-v1 ceiling");
            return None;
        }
        if fold.loop_entries > FULL_V1_MAX_LOOP_ENTRIES {
            self.ceiling(path, "kind", "loop entries exceed full-v1 ceiling");
            return None;
        }
        Some(fold)
    }

    fn checked_sum(
        &mut self,
        values: impl Iterator<Item = u64>,
        ceiling: u64,
        path: &[DiagnosticPathSegment],
        field: &str,
        label: &str,
    ) -> Option<u64> {
        let mut total = 0_u64;
        for value in values {
            total = match total.checked_add(value) {
                Some(total) if total <= ceiling => total,
                _ => {
                    self.ceiling(path, field, format!("{label} exceed {ceiling}"));
                    return None;
                }
            };
        }
        Some(total)
    }

    #[allow(clippy::too_many_arguments)]
    fn checked_product(
        &mut self,
        left: u64,
        right: u64,
        ceiling: u64,
        path: &[DiagnosticPathSegment],
        field: &str,
        label: &str,
    ) -> Option<u64> {
        match left.checked_mul(right) {
            Some(product) if product <= ceiling => Some(product),
            _ => {
                self.ceiling(path, field, format!("{label} exceed {ceiling}"));
                None
            }
        }
    }

    fn ceiling(&mut self, path: &[DiagnosticPathSegment], field: &str, message: impl Into<String>) {
        self.emit(
            GraphDiagnosticCode::CeilingExceeded,
            message,
            with_field(path, field),
            Vec::new(),
        );
    }

    fn validate_terminal_coverage(&mut self) {
        self.terminal_flow(&self.graph.root);
        if self.may_fall_through(&self.graph.root) {
            self.emit(
                GraphDiagnosticCode::Reachability,
                "normal-success path falls through without explicit succeed or fail terminal",
                vec![field_segment("root")],
                vec![self.graph.root.name().clone()],
            );
        }
    }

    fn terminal_flow(&mut self, node: &GraphNode) {
        match node {
            GraphNode::Seq(group) => {
                let mut reachable = true;
                for child in group.children.as_slice() {
                    if !reachable {
                        self.emit(
                            GraphDiagnosticCode::Reachability,
                            "node is unreachable after an unconditional terminal",
                            self.node_path(child.name()),
                            vec![child.name().clone(), group.name.clone()],
                        );
                    }
                    self.terminal_flow(child);
                    reachable &= self.may_fall_through(child);
                }
            }
            GraphNode::Choice(group) => {
                let reachability = self.choice_reachability.get(&group.name).cloned();
                for (index, branch) in group.branches.as_slice().iter().enumerate() {
                    if reachability
                        .as_ref()
                        .is_none_or(|control| control.branch_reachable(index))
                    {
                        self.terminal_flow(&branch.node);
                    }
                }
                if let Some(otherwise) = &group.otherwise {
                    if reachability
                        .as_ref()
                        .is_none_or(|control| control.otherwise_reachable)
                    {
                        self.terminal_flow(otherwise);
                    }
                }
            }
            GraphNode::Par(group) => {
                for branch in group.branches.as_slice() {
                    self.terminal_flow(branch);
                }
            }
            GraphNode::Loop(group) => self.terminal_flow(&group.body),
            GraphNode::Map(group) => self.terminal_flow(&group.body),
            GraphNode::Step(_)
            | GraphNode::Verifier(_)
            | GraphNode::Succeed(_)
            | GraphNode::Fail(_) => {}
        }
    }

    fn may_fall_through(&self, node: &GraphNode) -> bool {
        self.node_fallthrough
            .get(&node_identity(node))
            .copied()
            .unwrap_or(true)
    }

    fn reference_topological_order(&mut self) -> Vec<NodeName> {
        let known = self.nodes.keys().cloned().collect::<BTreeSet<_>>();
        self.validate_reference_names(&known);
        let mut remaining = known;
        let mut order = Vec::new();
        while !remaining.is_empty() {
            let Some(next) = self.next_reference_node(&remaining, &order) else {
                self.emit_reference_cycle(&remaining);
                break;
            };
            remaining.remove(&next);
            order.push(next);
        }
        order
    }

    fn validate_reference_names(&mut self, known: &BTreeSet<NodeName>) {
        for (node, dependencies) in self.dependencies.clone() {
            for dependency in dependencies {
                if !known.contains(&dependency) {
                    self.emit(
                        GraphDiagnosticCode::UndefinedRead,
                        format!("reference names unknown node {dependency}"),
                        self.node_path(&node),
                        vec![dependency],
                    );
                }
            }
        }
    }

    fn next_reference_node(
        &self,
        remaining: &BTreeSet<NodeName>,
        order: &[NodeName],
    ) -> Option<NodeName> {
        remaining
            .iter()
            .filter(|node| {
                self.dependencies.get(*node).is_none_or(|dependencies| {
                    dependencies
                        .iter()
                        .filter(|dependency| **dependency != **node)
                        .all(|dependency| order.contains(dependency))
                })
            })
            .min_by_key(|node| {
                self.nodes
                    .get(*node)
                    .map_or(usize::MAX, |info| info.ordinal)
            })
            .cloned()
    }

    fn emit_reference_cycle(&mut self, remaining: &BTreeSet<NodeName>) {
        let related = remaining.iter().cloned().collect::<Vec<_>>();
        let path = related
            .first()
            .map_or_else(|| vec![field_segment("root")], |name| self.node_path(name));
        self.emit(
            GraphDiagnosticCode::CyclicReference,
            "node-output/control references contain a cycle",
            path,
            related,
        );
    }

    fn worker_diagnostic(&self, diagnostic: WorkerCompatibilityDiagnostic) -> GraphDiagnostic {
        let code = match diagnostic.code {
            WorkerCompatibilityCode::Registry
            | WorkerCompatibilityCode::DescriptorContract
            | WorkerCompatibilityCode::DescriptorIdentity
            | WorkerCompatibilityCode::GraphProfile
            | WorkerCompatibilityCode::VerifierContract => GraphDiagnosticCode::InvalidGraphShape,
            WorkerCompatibilityCode::Input
            | WorkerCompatibilityCode::Output
            | WorkerCompatibilityCode::SignalField
            | WorkerCompatibilityCode::SignalLabels
            | WorkerCompatibilityCode::Diagnostic => GraphDiagnosticCode::SchemaSafety,
        };
        let node = diagnostic
            .path
            .last()
            .and_then(|name| NodeName::new(name.clone()).ok());
        let path = node
            .as_ref()
            .map_or_else(|| vec![field_segment("root")], |name| self.node_path(name));
        GraphDiagnostic {
            severity: DiagnosticSeverity::Error,
            code,
            message: diagnostic.message,
            path,
            related_nodes: node.into_iter().collect(),
        }
    }

    fn node_path(&self, name: &NodeName) -> Vec<DiagnosticPathSegment> {
        self.nodes
            .get(name)
            .map_or_else(|| vec![field_segment("root")], |info| info.path.clone())
    }

    fn emit(
        &mut self,
        code: GraphDiagnosticCode,
        message: impl Into<String>,
        path: Vec<DiagnosticPathSegment>,
        mut related_nodes: Vec<NodeName>,
    ) {
        related_nodes.sort();
        related_nodes.dedup();
        self.diagnostics.push(GraphDiagnostic {
            severity: DiagnosticSeverity::Error,
            code,
            message: message.into(),
            path,
            related_nodes,
        });
    }

    fn sort_diagnostics(&mut self) {
        sort_diagnostics(&mut self.diagnostics);
    }
}

#[derive(Clone, Copy)]
enum PromotionRule {
    Definite,
    EveryAlternative,
    Map,
}

#[derive(Clone, Default)]
struct OutcomeRefinement {
    success: BTreeSet<NodeName>,
    failed: BTreeSet<NodeName>,
}

impl OutcomeRefinement {
    fn apply(&self, flow: &mut Flow) {
        for name in &self.success {
            flow.failed.remove(name);
        }
        flow.failed.extend(self.failed.clone());
        flow.resolve_successful_writes();
    }
}

struct ChoiceControl {
    branches: Vec<OutcomeRefinement>,
    branch_reachable: Vec<bool>,
    branch_completion: Vec<CompletionPredicate>,
    otherwise: OutcomeRefinement,
    otherwise_reachable: bool,
    otherwise_completion: CompletionPredicate,
    exhaustive: bool,
}

impl ChoiceControl {
    fn unknown(choice: &ChoiceNode) -> Self {
        Self {
            branches: vec![OutcomeRefinement::default(); choice.branches.as_slice().len()],
            branch_reachable: vec![true; choice.branches.as_slice().len()],
            branch_completion: vec![CompletionPredicate::Always; choice.branches.as_slice().len()],
            otherwise: OutcomeRefinement::default(),
            otherwise_reachable: choice.otherwise.is_some(),
            otherwise_completion: CompletionPredicate::Always,
            exhaustive: choice.otherwise.is_some(),
        }
    }

    fn reachability(&self) -> ChoiceReachability {
        ChoiceReachability {
            branches: self.branch_reachable.clone(),
            otherwise_reachable: self.otherwise_reachable,
        }
    }
}

fn choice_branch_completion_predicates(choice: &ChoiceNode) -> Vec<CompletionPredicate> {
    let mut earlier = Vec::new();
    choice
        .branches
        .as_slice()
        .iter()
        .map(|branch| {
            let current = CompletionPredicate::Guard(branch.when.clone());
            let residual = CompletionPredicate::all(
                earlier
                    .iter()
                    .cloned()
                    .map(CompletionPredicate::not)
                    .chain(std::iter::once(current.clone())),
            );
            earlier.push(current);
            residual
        })
        .collect()
}

fn choice_otherwise_completion_predicate(choice: &ChoiceNode) -> CompletionPredicate {
    CompletionPredicate::all(
        choice.branches.as_slice().iter().map(|branch| {
            CompletionPredicate::not(CompletionPredicate::Guard(branch.when.clone()))
        }),
    )
}

#[derive(Clone)]
struct ChoiceReachability {
    branches: Vec<bool>,
    otherwise_reachable: bool,
}

impl ChoiceReachability {
    fn branch_reachable(&self, index: usize) -> bool {
        self.branches.get(index).copied().unwrap_or(true)
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SelectorKey {
    name: NodeName,
    source: ControlSourceKey,
    field: Option<FieldName>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ControlSourceKey {
    Signal,
    Error,
    Group,
}

impl From<&ControlSelector> for SelectorKey {
    fn from(selector: &ControlSelector) -> Self {
        Self {
            name: selector.name.clone(),
            source: match selector.source {
                ControlSource::Signal => ControlSourceKey::Signal,
                ControlSource::Error => ControlSourceKey::Error,
                ControlSource::Group => ControlSourceKey::Group,
            },
            field: selector.field.clone(),
        }
    }
}

#[derive(Clone)]
struct AssignedControl {
    counts: BTreeMap<EnumLabel, u64>,
}

type Assignment = BTreeMap<SelectorKey, AssignedControl>;

fn outcome_refinement(assignments: &[&Assignment]) -> OutcomeRefinement {
    let names = assignments
        .iter()
        .flat_map(|assignment| assignment.keys())
        .filter(|key| {
            matches!(
                key.source,
                ControlSourceKey::Signal | ControlSourceKey::Error
            )
        })
        .map(|key| key.name.clone())
        .collect::<BTreeSet<_>>();
    let mut refinement = OutcomeRefinement::default();
    for name in names {
        if assignments
            .iter()
            .any(|assignment| assignment_has_error(assignment, &name))
        {
            refinement.failed.insert(name);
        } else {
            refinement.success.insert(name);
        }
    }
    refinement
}

fn assignment_has_error(assignment: &Assignment, name: &NodeName) -> bool {
    let mut has_signal = false;
    let mut signal_occurrences = 0_u64;
    for (key, value) in assignment.iter().filter(|(key, _)| key.name == *name) {
        match key.source {
            ControlSourceKey::Error if value.counts.values().any(|count| *count > 0) => {
                return true;
            }
            ControlSourceKey::Signal => {
                has_signal = true;
                signal_occurrences =
                    signal_occurrences.saturating_add(value.counts.values().copied().sum::<u64>());
            }
            ControlSourceKey::Error | ControlSourceKey::Group => {}
        }
    }
    has_signal && signal_occurrences == 0
}

fn build_dimensions(
    selectors: &[ControlSelector],
    nodes: &BTreeMap<NodeName, NodeInfo<'_>>,
    map_aggregates: &BTreeMap<SelectorKey, u64>,
) -> Option<Vec<Vec<Assignment>>> {
    let mut by_node = BTreeMap::<NodeName, BTreeSet<SelectorKey>>::new();
    for selector in selectors {
        by_node
            .entry(selector.name.clone())
            .or_default()
            .insert(SelectorKey::from(selector));
    }
    let mut dimensions = Vec::new();
    for (name, keys) in by_node {
        let Some(node) = nodes.get(&name).map(|info| info.node) else {
            continue;
        };
        dimensions.extend(dimensions_for_node(node, &keys, map_aggregates)?);
    }
    Some(dimensions)
}

fn enumerate_assignments(dimensions: Vec<Vec<Assignment>>) -> Vec<Assignment> {
    let mut assignments = vec![Assignment::new()];
    for dimension in dimensions {
        let mut next = Vec::with_capacity(assignments.len() * dimension.len());
        for prefix in &assignments {
            for choice in &dimension {
                let mut assignment = prefix.clone();
                assignment.extend(choice.clone());
                next.push(assignment);
            }
        }
        assignments = next;
    }
    assignments
}

fn dimensions_for_node(
    node: &GraphNode,
    keys: &BTreeSet<SelectorKey>,
    map_aggregates: &BTreeMap<SelectorKey, u64>,
) -> Option<Vec<Vec<Assignment>>> {
    let aggregate = keys
        .iter()
        .filter(|key| {
            map_aggregates.contains_key(*key)
                && matches!(
                    key.source,
                    ControlSourceKey::Signal | ControlSourceKey::Error
                )
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut dimensions = Vec::new();
    if !aggregate.is_empty() {
        let maximum = aggregate
            .iter()
            .filter_map(|key| map_aggregates.get(key))
            .copied()
            .min()?;
        dimensions.push(aggregate_outcome_dimension(node, &aggregate, maximum)?);
    }
    let scalar = keys
        .iter()
        .filter(|key| !aggregate.contains(key))
        .cloned()
        .collect::<Vec<_>>();
    dimensions.extend(group_control_dimensions(node, &scalar));
    let outcomes = scalar
        .into_iter()
        .filter(|key| key.source != ControlSourceKey::Group)
        .collect::<Vec<_>>();
    if !outcomes.is_empty() {
        dimensions.push(single_outcome_dimension(node, &outcomes)?);
    }
    Some(dimensions)
}

fn group_control_dimensions(node: &GraphNode, keys: &[SelectorKey]) -> Vec<Vec<Assignment>> {
    keys.iter()
        .filter(|key| key.source == ControlSourceKey::Group)
        .map(|key| {
            group_domain(node, key.field.as_ref())
                .unwrap_or_default()
                .into_iter()
                .map(|label| BTreeMap::from([(key.clone(), assigned_control(Some(label), 1))]))
                .collect()
        })
        .collect()
}

fn single_outcome_dimension(node: &GraphNode, keys: &[SelectorKey]) -> Option<Vec<Assignment>> {
    let choices = per_execution_outcomes(node, keys)?;
    (choices.len() as u64 <= FULL_V1_MAX_GUARD_ASSIGNMENTS).then_some(choices)
}

fn aggregate_outcome_dimension(
    node: &GraphNode,
    keys: &[SelectorKey],
    maximum: u64,
) -> Option<Vec<Assignment>> {
    let outcomes = per_execution_outcomes(node, keys)?;
    let count = bounded_distribution_count(maximum, outcomes.len())?;
    if count > FULL_V1_MAX_GUARD_ASSIGNMENTS {
        return None;
    }
    Some(
        enumerate_count_vectors(outcomes.len(), maximum)
            .into_iter()
            .map(|counts| combine_outcome_counts(keys, &outcomes, &counts))
            .collect(),
    )
}

fn per_execution_outcomes(node: &GraphNode, keys: &[SelectorKey]) -> Option<Vec<Assignment>> {
    let signal_keys = keys
        .iter()
        .filter(|key| key.source == ControlSourceKey::Signal)
        .cloned()
        .collect::<Vec<_>>();
    let error_keys = keys
        .iter()
        .filter(|key| key.source == ControlSourceKey::Error)
        .cloned()
        .collect::<Vec<_>>();
    let mut successful = vec![Assignment::new()];
    for key in &signal_keys {
        let labels = selector_key_domain(key, node);
        successful = cartesian_signal_outcomes(successful, key, &labels);
        if successful.len() as u64 > FULL_V1_MAX_GUARD_ASSIGNMENTS {
            return None;
        }
    }
    for assignment in &mut successful {
        insert_empty_controls(assignment, &error_keys);
    }
    let mut outcomes = successful;
    for error in error_labels() {
        let mut assignment = Assignment::new();
        insert_empty_controls(&mut assignment, &signal_keys);
        for key in &error_keys {
            assignment.insert(key.clone(), assigned_control(Some(error.clone()), 1));
        }
        outcomes.push(assignment);
    }
    Some(outcomes)
}

fn cartesian_signal_outcomes(
    prefixes: Vec<Assignment>,
    key: &SelectorKey,
    labels: &[EnumLabel],
) -> Vec<Assignment> {
    prefixes
        .into_iter()
        .flat_map(|prefix| {
            labels.iter().map(move |label| {
                let mut choice = prefix.clone();
                choice.insert(key.clone(), assigned_control(Some(label.clone()), 1));
                choice
            })
        })
        .collect()
}

fn insert_empty_controls(assignment: &mut Assignment, keys: &[SelectorKey]) {
    for key in keys {
        assignment.insert(key.clone(), assigned_control(None, 0));
    }
}

fn combine_outcome_counts(
    keys: &[SelectorKey],
    outcomes: &[Assignment],
    counts: &[u64],
) -> Assignment {
    let mut combined = keys
        .iter()
        .cloned()
        .map(|key| (key, assigned_control(None, 0)))
        .collect::<Assignment>();
    for (outcome, occurrences) in outcomes.iter().zip(counts) {
        for (key, value) in outcome {
            let target = combined
                .entry(key.clone())
                .or_insert_with(|| assigned_control(None, 0));
            for (label, count) in &value.counts {
                *target.counts.entry(label.clone()).or_default() += count * occurrences;
            }
        }
    }
    combined
}

fn evaluate_guard(guard: &Guard, assignment: &Assignment) -> bool {
    match guard {
        Guard::In { value, labels } => selector_matches(value, labels.values(), assignment),
        Guard::All { guards } => guards
            .as_slice()
            .iter()
            .all(|guard| evaluate_guard(guard, assignment)),
        Guard::Any { guards } => guards
            .as_slice()
            .iter()
            .any(|guard| evaluate_guard(guard, assignment)),
        Guard::Not { guard } => !evaluate_guard(guard, assignment),
        Guard::KOfN {
            count,
            values,
            labels,
        } => {
            values
                .as_slice()
                .iter()
                .filter(|selector| selector_matches(selector, labels.values(), assignment))
                .count() as u64
                >= count.get()
        }
        Guard::KOfMap {
            count,
            value,
            labels,
        } => selector_occurrences(value, labels.values(), assignment) >= count.get(),
    }
}

fn selector_matches(
    selector: &ControlSelector,
    labels: &[EnumLabel],
    assignment: &Assignment,
) -> bool {
    assignment
        .get(&SelectorKey::from(selector))
        .is_some_and(|value| {
            labels
                .iter()
                .any(|label| value.counts.get(label).is_some_and(|count| *count > 0))
        })
}

fn selector_occurrences(
    selector: &ControlSelector,
    labels: &[EnumLabel],
    assignment: &Assignment,
) -> u64 {
    assignment
        .get(&SelectorKey::from(selector))
        .map_or(0, |value| {
            labels
                .iter()
                .filter_map(|label| value.counts.get(label))
                .copied()
                .sum()
        })
}

fn assigned_control(label: Option<EnumLabel>, occurrences: u64) -> AssignedControl {
    AssignedControl {
        counts: label
            .map(|label| BTreeMap::from([(label, occurrences)]))
            .unwrap_or_default(),
    }
}

fn bounded_distribution_count(maximum: u64, labels: usize) -> Option<u64> {
    // Weak compositions of at most `maximum` occurrences across `labels` values:
    // C(maximum + labels, labels). Stop as soon as the verifier ceiling is crossed.
    let labels = u64::try_from(labels).ok()?;
    let mut result = 1_u64;
    for divisor in 1..=labels {
        let numerator = maximum.checked_add(divisor)?;
        result = result.checked_mul(numerator)?.checked_div(divisor)?;
        if result > FULL_V1_MAX_GUARD_ASSIGNMENTS {
            return Some(result);
        }
    }
    Some(result)
}

fn enumerate_count_vectors(dimensions: usize, maximum: u64) -> Vec<Vec<u64>> {
    fn visit(
        dimensions: usize,
        index: usize,
        remaining: u64,
        counts: &mut Vec<u64>,
        output: &mut Vec<Vec<u64>>,
    ) {
        if index == dimensions {
            output.push(counts.clone());
            return;
        }
        for count in 0..=remaining {
            counts.push(count);
            visit(dimensions, index + 1, remaining - count, counts, output);
            counts.pop();
        }
    }

    let mut output = Vec::new();
    visit(dimensions, 0, maximum, &mut Vec::new(), &mut output);
    output
}

fn selector_key_domain(key: &SelectorKey, node: &GraphNode) -> Vec<EnumLabel> {
    match key.source {
        ControlSourceKey::Signal => match node {
            GraphNode::Verifier(verifier) => key
                .field
                .as_ref()
                .and_then(|field| verifier.signals.get(field))
                .map(|labels| labels.values().to_vec())
                .unwrap_or_default(),
            _ => Vec::new(),
        },
        ControlSourceKey::Error => error_labels(),
        ControlSourceKey::Group => group_domain(node, key.field.as_ref()).unwrap_or_default(),
    }
}

fn group_domain(node: &GraphNode, field: Option<&FieldName>) -> Option<Vec<EnumLabel>> {
    let expected = match node {
        GraphNode::Loop(_) if field.is_some_and(|field| field.as_str() == "terminated") => {
            &["converged", "exhausted"][..]
        }
        GraphNode::Map(_) if field.is_some_and(|field| field.as_str() == "overflow") => {
            &["ok", "overflow"][..]
        }
        GraphNode::Par(par)
            if matches!(par.join, Join::All {} | Join::Any {} | Join::Quorum { .. })
                && field.is_some_and(|field| field.as_str() == "joined") =>
        {
            &["reached", "quorum_unreachable"][..]
        }
        GraphNode::Par(par)
            if matches!(par.join, Join::First { .. })
                && field.is_some_and(|field| field.as_str() == "raced") =>
        {
            &["satisfied", "no_satisfier"][..]
        }
        _ => return None,
    };
    Some(expected.iter().map(|label| enum_label(label)).collect())
}

fn error_labels() -> Vec<EnumLabel> {
    ["timeout", "crash", "malformed", "refusal"]
        .into_iter()
        .map(enum_label)
        .collect()
}

fn signal_path_type(verifier: &VerifierNode, path: &FieldPath) -> Option<PayloadType> {
    let [field] = path.segments() else {
        return None;
    };
    verifier
        .signals
        .get(field)
        .cloned()
        .map(|values| PayloadType::Enum { values })
}

fn executable_output(node: &GraphNode) -> Option<&PayloadType> {
    match node {
        GraphNode::Step(step) => Some(&step.output),
        GraphNode::Verifier(verifier) => Some(&verifier.output),
        _ => None,
    }
}

fn node_state(node: &GraphNode) -> Option<&PayloadType> {
    match node {
        GraphNode::Seq(group) => Some(&group.state),
        GraphNode::Choice(group) => Some(&group.state),
        GraphNode::Par(group) => Some(&group.state),
        GraphNode::Loop(group) => Some(&group.state),
        GraphNode::Map(group) => Some(&group.state),
        GraphNode::Step(_)
        | GraphNode::Verifier(_)
        | GraphNode::Succeed(_)
        | GraphNode::Fail(_) => None,
    }
}

fn required_leaf_paths(payload: &PayloadType) -> Vec<FieldPath> {
    fn collect(
        payload: &PayloadType,
        prefix: &mut Vec<FieldName>,
        required: bool,
        paths: &mut Vec<FieldPath>,
    ) -> bool {
        if !required {
            return false;
        }
        let PayloadType::Record { fields } = payload else {
            if !prefix.is_empty() {
                paths.push(
                    FieldPath::new(prefix.clone())
                        .expect("non-empty payload traversal path is valid"),
                );
                return true;
            }
            return false;
        };
        let mut has_required_descendant = false;
        for (name, field) in fields {
            prefix.push(name.clone());
            has_required_descendant |= collect(&field.value_type, prefix, field.required, paths);
            prefix.pop();
        }
        if !has_required_descendant && !prefix.is_empty() {
            paths.push(
                FieldPath::new(prefix.clone()).expect("non-empty payload traversal path is valid"),
            );
            return true;
        }
        has_required_descendant
    }

    let mut paths = Vec::new();
    collect(payload, &mut Vec::new(), true, &mut paths);
    paths
}

fn required_paths_with_types(payload: &PayloadType) -> BTreeMap<FieldPath, PayloadType> {
    fn collect(
        payload: &PayloadType,
        prefix: &mut Vec<FieldName>,
        paths: &mut BTreeMap<FieldPath, PayloadType>,
    ) {
        let PayloadType::Record { fields } = payload else {
            return;
        };
        for (name, field) in fields {
            if !field.required {
                continue;
            }
            prefix.push(name.clone());
            if let Ok(path) = FieldPath::new(prefix.clone()) {
                paths.insert(path, field.value_type.clone());
                collect(&field.value_type, prefix, paths);
            }
            prefix.pop();
        }
    }

    let mut paths = BTreeMap::new();
    collect(payload, &mut Vec::new(), &mut paths);
    paths
}

fn guaranteed_write_paths(
    target: &FieldPath,
    value_type: &PayloadType,
    definitely_present: bool,
) -> BTreeMap<FieldPath, PayloadType> {
    if !definitely_present {
        return BTreeMap::new();
    }
    let mut paths = BTreeMap::from([(target.clone(), value_type.clone())]);
    let mut prefix = target.segments().to_vec();
    collect_required_descendant_paths(value_type, &mut prefix, &mut paths);
    paths
}

fn collect_required_descendant_paths(
    payload: &PayloadType,
    prefix: &mut Vec<FieldName>,
    paths: &mut BTreeMap<FieldPath, PayloadType>,
) {
    let PayloadType::Record { fields } = payload else {
        return;
    };
    for (name, field) in fields {
        if !field.required {
            continue;
        }
        prefix.push(name.clone());
        if let Ok(path) = FieldPath::new(prefix.clone()) {
            paths.insert(path, field.value_type.clone());
            collect_required_descendant_paths(&field.value_type, prefix, paths);
        }
        prefix.pop();
    }
}

fn display_field_path(path: &FieldPath) -> String {
    path.segments()
        .iter()
        .map(FieldName::as_str)
        .collect::<Vec<_>>()
        .join(".")
}

fn path_type<'a>(payload: &'a PayloadType, path: &FieldPath) -> Option<&'a PayloadType> {
    let mut current = payload;
    for segment in path.segments() {
        let PayloadType::Record { fields } = current else {
            return None;
        };
        current = &fields.get(segment)?.value_type;
    }
    Some(current)
}

fn selected_output_path(payload: &PayloadType, path: &FieldPath) -> Option<SelectedOutput> {
    path_type(payload, path)
        .cloned()
        .map(|value_type| SelectedOutput {
            value_type,
            definitely_present: is_required_path(payload, path),
        })
}

fn is_subtype_with_definitions(
    source: &PayloadType,
    target: &PayloadType,
    definitions: &BTreeMap<FieldPath, PayloadType>,
) -> bool {
    fn check(
        source: &PayloadType,
        target: &PayloadType,
        definitions: &BTreeMap<FieldPath, PayloadType>,
        prefix: &mut Vec<FieldName>,
    ) -> bool {
        let (PayloadType::Record { fields: source }, PayloadType::Record { fields: target }) =
            (source, target)
        else {
            return source.is_subtype_of(target);
        };
        target.iter().all(|(name, target_field)| {
            let Some(source_field) = source.get(name) else {
                return !target_field.required;
            };
            prefix.push(name.clone());
            let path = FieldPath::new(prefix.clone()).ok();
            let refined = path.as_ref().and_then(|path| definitions.get(path));
            let present = source_field.required || refined.is_some();
            let compatible = (!target_field.required || present)
                && check(
                    refined.unwrap_or(&source_field.value_type),
                    &target_field.value_type,
                    definitions,
                    prefix,
                );
            prefix.pop();
            compatible
        })
    }

    check(source, target, definitions, &mut Vec::new())
}

fn is_required_path(payload: &PayloadType, path: &FieldPath) -> bool {
    let mut current = payload;
    for segment in path.segments() {
        let PayloadType::Record { fields } = current else {
            return false;
        };
        let Some(field) = fields.get(segment) else {
            return false;
        };
        if !field.required {
            return false;
        }
        current = &field.value_type;
    }
    true
}

fn is_defined(defined: &BTreeMap<FieldPath, PayloadType>, selected: &FieldPath) -> bool {
    defined.contains_key(selected)
}

fn paths_overlap(left: &FieldPath, right: &FieldPath) -> bool {
    path_is_prefix(left, right) || path_is_prefix(right, left)
}

fn path_is_prefix(prefix: &FieldPath, path: &FieldPath) -> bool {
    prefix.segments().len() <= path.segments().len()
        && prefix
            .segments()
            .iter()
            .zip(path.segments())
            .all(|(left, right)| left == right)
}

fn terminal_effects(name: &NodeName, incoming: &Flow) -> Effects {
    Effects {
        definite_nodes: BTreeSet::from([name.clone()]),
        exit_failed: incoming.failed.clone(),
        ..Effects::default()
    }
}

fn node_identity(node: &GraphNode) -> usize {
    std::ptr::from_ref(node).cast::<()>() as usize
}

fn parallel_required_completions(join: &Join, branch_count: usize) -> u64 {
    match join {
        Join::All {} => branch_count as u64,
        Join::Any {} | Join::First { .. } => 1,
        Join::Quorum { count } => count.get(),
    }
}

fn merge_parallel_scenarios(
    branches: &[Effects],
    required: u64,
    scenarios: &[BTreeSet<usize>],
) -> Effects {
    let mut merged = Effects::default();
    for branch in branches {
        merged
            .possible_writes
            .extend(branch.possible_writes.clone());
        merge_possible_write_types(
            &mut merged.possible_write_types,
            &branch.possible_write_types,
        );
    }

    if !scenarios.is_empty() {
        merged.definite_nodes = parallel_definite_nodes(branches, required, scenarios);
        merged.definite_writes = parallel_definite_writes(branches, required, scenarios);
        merged.outcome_writes = parallel_outcome_writes(branches, required, scenarios);
        merged.exit_failed = scenarios
            .iter()
            .flat_map(|scenario| scenario.iter())
            .flat_map(|index| branches[*index].exit_failed.clone())
            .collect();
        merged.falls_through = true;
    }
    merged
}

fn parallel_definite_nodes(
    branches: &[Effects],
    required: u64,
    scenarios: &[BTreeSet<usize>],
) -> BTreeSet<NodeName> {
    let candidates = branches
        .iter()
        .flat_map(|branch| branch.definite_nodes.iter().cloned())
        .collect::<BTreeSet<_>>();
    candidates
        .into_iter()
        .filter(|name| {
            scenarios.iter().all(|scenario| {
                parallel_fact_is_definite(scenario, required, |index| {
                    branches[index].definite_nodes.contains(name)
                })
            })
        })
        .collect()
}

fn parallel_definite_writes(
    branches: &[Effects],
    required: u64,
    scenarios: &[BTreeSet<usize>],
) -> Writes {
    let candidates = branches
        .iter()
        .flat_map(|branch| branch.definite_writes.keys().cloned())
        .collect::<BTreeSet<_>>();
    candidates
        .into_iter()
        .filter_map(|path| {
            let guaranteed = scenarios.iter().all(|scenario| {
                parallel_fact_is_definite(scenario, required, |index| {
                    branches[index].definite_writes.contains_key(&path)
                })
            });
            let providers = scenarios
                .iter()
                .flat_map(|scenario| scenario.iter())
                .filter_map(|index| branches[*index].definite_writes.get(&path))
                .collect::<Vec<_>>();
            guaranteed
                .then(|| intersect_write_fact_set(&providers))
                .flatten()
                .map(|write| (path, write))
        })
        .collect()
}

fn parallel_outcome_writes(
    branches: &[Effects],
    required: u64,
    scenarios: &[BTreeSet<usize>],
) -> OutcomeWrites {
    let candidates = branches
        .iter()
        .flat_map(|branch| {
            branch
                .outcome_writes
                .iter()
                .flat_map(|(name, writes)| writes.keys().cloned().map(|path| (name.clone(), path)))
        })
        .collect::<BTreeSet<_>>();
    let mut merged = OutcomeWrites::new();
    for (name, path) in candidates {
        let guaranteed = scenarios.iter().all(|scenario| {
            parallel_fact_is_definite(scenario, required, |index| {
                branches[index]
                    .outcome_writes
                    .get(&name)
                    .is_some_and(|writes| writes.contains_key(&path))
            })
        });
        let providers = scenarios
            .iter()
            .flat_map(|scenario| scenario.iter())
            .filter_map(|index| {
                branches[*index]
                    .outcome_writes
                    .get(&name)
                    .and_then(|writes| writes.get(&path))
            })
            .collect::<Vec<_>>();
        if guaranteed {
            if let Some(write) = intersect_write_fact_set(&providers) {
                merged.entry(name.clone()).or_default().insert(path, write);
            }
        }
    }
    merged
}

fn parallel_fact_is_definite(
    completing: &BTreeSet<usize>,
    required: u64,
    provides: impl Fn(usize) -> bool,
) -> bool {
    (completing.iter().filter(|index| !provides(**index)).count() as u64) < required
}

fn intersect_write_fact_set(writes: &[&WriteFact]) -> Option<WriteFact> {
    let (first, rest) = writes.split_first()?;
    rest.iter().try_fold((*first).clone(), |common, write| {
        intersect_write_facts(&common, write)
    })
}

fn merge_alternatives(branches: &[Effects]) -> Effects {
    let completing = branches
        .iter()
        .filter(|branch| branch.falls_through)
        .collect::<Vec<_>>();
    let Some(first) = completing.first() else {
        let mut possible_write_types = BTreeMap::new();
        for branch in branches {
            merge_possible_write_types(&mut possible_write_types, &branch.possible_write_types);
        }
        return Effects {
            possible_writes: branches
                .iter()
                .flat_map(|branch| branch.possible_writes.clone())
                .collect(),
            possible_write_types,
            ..Effects::default()
        };
    };
    let mut definite_nodes = first.definite_nodes.clone();
    let mut definite_writes = first.definite_writes.clone();
    let mut outcome_writes = first.outcome_writes.clone();
    let mut possible_writes = BTreeMap::new();
    let mut possible_write_types = BTreeMap::new();
    let mut exit_failed = BTreeSet::new();
    let completion =
        CompletionPredicate::any(completing.iter().map(|branch| branch.completion.clone()));
    for branch in branches {
        possible_writes.extend(branch.possible_writes.clone());
        merge_possible_write_types(&mut possible_write_types, &branch.possible_write_types);
        if branch.falls_through {
            retain_common_effects(
                &mut definite_nodes,
                &mut definite_writes,
                &mut outcome_writes,
                branch,
            );
            exit_failed.extend(branch.exit_failed.clone());
        }
    }
    Effects {
        definite_nodes,
        definite_writes,
        possible_writes,
        possible_write_types,
        outcome_writes,
        exit_failed,
        falls_through: true,
        completion,
    }
}

fn retain_common_effects(
    definite_nodes: &mut BTreeSet<NodeName>,
    definite_writes: &mut Writes,
    outcome_writes: &mut OutcomeWrites,
    branch: &Effects,
) {
    definite_nodes.retain(|name| branch.definite_nodes.contains(name));
    definite_writes.retain(|path, write| {
        let Some(other) = branch.definite_writes.get(path) else {
            return false;
        };
        let Some(common) = intersect_write_facts(write, other) else {
            return false;
        };
        *write = common;
        true
    });
    outcome_writes.retain(|name, writes| {
        let Some(other) = branch.outcome_writes.get(name) else {
            return false;
        };
        writes.retain(|path, write| {
            let Some(other) = other.get(path) else {
                return false;
            };
            let Some(common) = intersect_write_facts(write, other) else {
                return false;
            };
            *write = common;
            true
        });
        !writes.is_empty()
    });
}

fn intersect_write_facts(left: &WriteFact, right: &WriteFact) -> Option<WriteFact> {
    let value_type = common_supertype(&left.value_type, &right.value_type)?;
    let mut guaranteed_paths = BTreeMap::new();
    for (path, left_type) in &left.guaranteed_paths {
        let Some(right_type) = right.guaranteed_paths.get(path) else {
            continue;
        };
        if let Some(value_type) = common_supertype(left_type, right_type) {
            guaranteed_paths.insert(path.clone(), value_type);
        }
    }
    Some(WriteFact {
        value_type,
        guaranteed_paths,
    })
}

fn common_supertype(left: &PayloadType, right: &PayloadType) -> Option<PayloadType> {
    if left.is_subtype_of(right) {
        Some(right.clone())
    } else if right.is_subtype_of(left) {
        Some(left.clone())
    } else {
        None
    }
}

fn apply_write_facts(defined: &mut BTreeMap<FieldPath, PayloadType>, writes: &Writes) {
    for (target, write) in writes {
        defined.retain(|path, _| !path_is_prefix(target, path));
        defined.extend(write.guaranteed_paths.clone());
    }
}

fn merge_outcome_writes(target: &mut OutcomeWrites, source: &OutcomeWrites) {
    for (name, writes) in source {
        target
            .entry(name.clone())
            .or_default()
            .extend(writes.clone());
    }
}

fn merge_possible_write_types(
    target: &mut BTreeMap<FieldPath, Vec<PayloadType>>,
    source: &BTreeMap<FieldPath, Vec<PayloadType>>,
) {
    for (path, source_types) in source {
        let target_types = target.entry(path.clone()).or_default();
        for source_type in source_types {
            if !target_types.contains(source_type) {
                target_types.push(source_type.clone());
            }
        }
    }
}

fn retain_promoted_writes(effects: &mut Effects, allowed: &BTreeSet<FieldPath>) {
    effects
        .definite_writes
        .retain(|path, _| allowed.contains(path));
    effects
        .possible_writes
        .retain(|path, _| allowed.contains(path));
    effects
        .possible_write_types
        .retain(|path, _| allowed.contains(path));
    for writes in effects.outcome_writes.values_mut() {
        writes.retain(|path, _| allowed.contains(path));
    }
    effects
        .outcome_writes
        .retain(|_, writes| !writes.is_empty());
}

fn compatible_types(left: &PayloadType, right: &PayloadType) -> bool {
    left.is_subtype_of(right) || right.is_subtype_of(left)
}

fn collect_descendant_names(branches: &NonEmptyVec<GraphNode>) -> BTreeSet<NodeName> {
    let mut names = BTreeSet::new();
    for branch in branches.as_slice() {
        collect_names(branch, &mut names);
    }
    names
}

fn collect_names(node: &GraphNode, names: &mut BTreeSet<NodeName>) {
    names.insert(node.name().clone());
    match node {
        GraphNode::Seq(group) => {
            for child in group.children.as_slice() {
                collect_names(child, names);
            }
        }
        GraphNode::Choice(group) => {
            for branch in group.branches.as_slice() {
                collect_names(&branch.node, names);
            }
            if let Some(otherwise) = &group.otherwise {
                collect_names(otherwise, names);
            }
        }
        GraphNode::Par(group) => {
            for branch in group.branches.as_slice() {
                collect_names(branch, names);
            }
        }
        GraphNode::Loop(group) => collect_names(&group.body, names),
        GraphNode::Map(group) => collect_names(&group.body, names),
        GraphNode::Step(_)
        | GraphNode::Verifier(_)
        | GraphNode::Succeed(_)
        | GraphNode::Fail(_) => {}
    }
}

fn find_map_owner<'a>(
    node: &'a GraphNode,
    target: &NodeName,
    owner: Option<&'a NodeName>,
) -> Option<&'a NodeName> {
    if node.name() == target {
        return owner;
    }
    match node {
        GraphNode::Seq(group) => group
            .children
            .as_slice()
            .iter()
            .find_map(|child| find_map_owner(child, target, owner)),
        GraphNode::Choice(group) => group
            .branches
            .as_slice()
            .iter()
            .find_map(|branch| find_map_owner(&branch.node, target, owner))
            .or_else(|| {
                group
                    .otherwise
                    .as_ref()
                    .and_then(|node| find_map_owner(node, target, owner))
            }),
        GraphNode::Par(group) => group
            .branches
            .as_slice()
            .iter()
            .find_map(|branch| find_map_owner(branch, target, owner)),
        GraphNode::Loop(group) => find_map_owner(&group.body, target, owner),
        GraphNode::Map(group) => find_map_owner(&group.body, target, Some(&group.name)),
        GraphNode::Step(_)
        | GraphNode::Verifier(_)
        | GraphNode::Succeed(_)
        | GraphNode::Fail(_) => None,
    }
}

fn guard_node_count(guard: &Guard) -> u64 {
    1 + match guard {
        Guard::All { guards } | Guard::Any { guards } => {
            guards.as_slice().iter().map(guard_node_count).sum()
        }
        Guard::Not { guard } => guard_node_count(guard),
        Guard::In { .. } | Guard::KOfN { .. } | Guard::KOfMap { .. } => 0,
    }
}

fn guard_selectors(guard: &Guard) -> Vec<&ControlSelector> {
    let mut selectors = Vec::new();
    collect_guard_selectors(guard, &mut selectors);
    selectors
}

fn guard_selector_uses(guard: &Guard) -> Vec<(&ControlSelector, bool)> {
    let mut selectors = Vec::new();
    collect_guard_selector_uses(guard, false, &mut selectors);
    selectors
}

fn collect_guard_selector_uses<'a>(
    guard: &'a Guard,
    map_aggregate: bool,
    selectors: &mut Vec<(&'a ControlSelector, bool)>,
) {
    match guard {
        Guard::In { value, .. } => selectors.push((value, map_aggregate)),
        Guard::KOfMap { value, .. } => selectors.push((value, true)),
        Guard::All { guards } | Guard::Any { guards } => {
            for guard in guards.as_slice() {
                collect_guard_selector_uses(guard, map_aggregate, selectors);
            }
        }
        Guard::Not { guard } => collect_guard_selector_uses(guard, map_aggregate, selectors),
        Guard::KOfN { values, .. } => {
            selectors.extend(values.as_slice().iter().map(|value| (value, map_aggregate)));
        }
    }
}

fn collect_guard_selectors<'a>(guard: &'a Guard, selectors: &mut Vec<&'a ControlSelector>) {
    match guard {
        Guard::In { value, .. } | Guard::KOfMap { value, .. } => selectors.push(value),
        Guard::All { guards } | Guard::Any { guards } => {
            for guard in guards.as_slice() {
                collect_guard_selectors(guard, selectors);
            }
        }
        Guard::Not { guard } => collect_guard_selectors(guard, selectors),
        Guard::KOfN { values, .. } => selectors.extend(values.as_slice()),
    }
}

fn sort_diagnostics(diagnostics: &mut [GraphDiagnostic]) {
    diagnostics.sort_by(|left, right| {
        compare_paths(&left.path, &right.path)
            .then_with(|| diagnostic_code_rank(left.code).cmp(&diagnostic_code_rank(right.code)))
            .then_with(|| left.message.cmp(&right.message))
            .then_with(|| left.related_nodes.cmp(&right.related_nodes))
    });
}

fn compare_paths(left: &[DiagnosticPathSegment], right: &[DiagnosticPathSegment]) -> Ordering {
    for (left, right) in left.iter().zip(right) {
        let ordering = path_segment_key(left).cmp(&path_segment_key(right));
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    left.len().cmp(&right.len())
}

fn path_segment_key(segment: &DiagnosticPathSegment) -> (u8, String, u32) {
    match segment {
        DiagnosticPathSegment::Field { name } => (0, name.as_str().to_owned(), 0),
        DiagnosticPathSegment::Index { index } => (1, String::new(), *index),
        DiagnosticPathSegment::Node { name } => (2, name.as_str().to_owned(), 0),
    }
}

const fn diagnostic_code_rank(code: GraphDiagnosticCode) -> u8 {
    match code {
        GraphDiagnosticCode::SchemaSafety => 0,
        GraphDiagnosticCode::Reachability => 1,
        GraphDiagnosticCode::ChoiceExhaustiveness => 2,
        GraphDiagnosticCode::LoopExitSatisfiability => 3,
        GraphDiagnosticCode::MissingBound => 4,
        GraphDiagnosticCode::WriteConflict => 5,
        GraphDiagnosticCode::CeilingExceeded => 6,
        GraphDiagnosticCode::CyclicReference => 7,
        GraphDiagnosticCode::UndefinedRead => 8,
        GraphDiagnosticCode::InvalidGraphShape => 9,
    }
}

fn field_name(value: &str) -> FieldName {
    FieldName::new(value).expect("static diagnostic field name must be valid")
}

fn enum_label(value: &str) -> EnumLabel {
    EnumLabel::new(value).expect("static control label must be valid")
}

fn field_segment(value: &str) -> DiagnosticPathSegment {
    DiagnosticPathSegment::Field {
        name: field_name(value),
    }
}

fn index_segment(value: usize) -> DiagnosticPathSegment {
    DiagnosticPathSegment::Index {
        index: u32::try_from(value).expect("graph collection index is wire-bounded"),
    }
}

fn node_segment(value: &NodeName) -> DiagnosticPathSegment {
    DiagnosticPathSegment::Node {
        name: value.clone(),
    }
}

fn with_field(path: &[DiagnosticPathSegment], field: &str) -> Vec<DiagnosticPathSegment> {
    let mut result = path.to_vec();
    result.push(field_segment(field));
    result
}

fn field_path(path: &[DiagnosticPathSegment], fields: &[&str]) -> Vec<DiagnosticPathSegment> {
    let mut result = path.to_vec();
    result.extend(fields.iter().map(|field| field_segment(field)));
    result
}

fn indexed_field_path(
    path: &[DiagnosticPathSegment],
    field: &str,
    index: usize,
) -> Vec<DiagnosticPathSegment> {
    let mut result = with_field(path, field);
    result.push(index_segment(index));
    result
}

fn child_path(
    path: &[DiagnosticPathSegment],
    field: &str,
    index: usize,
    name: &NodeName,
) -> Vec<DiagnosticPathSegment> {
    let mut result = indexed_field_path(path, field, index);
    result.push(node_segment(name));
    result
}

fn named_child_path(
    path: &[DiagnosticPathSegment],
    field: &str,
    name: &NodeName,
) -> Vec<DiagnosticPathSegment> {
    let mut result = with_field(path, field);
    result.push(node_segment(name));
    result
}

fn guard_path(
    path: &[DiagnosticPathSegment],
    collection: &str,
    index: usize,
    field: &str,
) -> Vec<DiagnosticPathSegment> {
    let mut result = indexed_field_path(path, collection, index);
    result.push(field_segment(field));
    result
}

fn choice_branch_node_path(
    path: &[DiagnosticPathSegment],
    index: usize,
    name: &NodeName,
) -> Vec<DiagnosticPathSegment> {
    let mut result = indexed_field_path(path, "branches", index);
    result.push(field_segment("node"));
    result.push(node_segment(name));
    result
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use openengine_cluster_protocol::{
        GraphSpec, NodeName, NonEmptyVec, PositiveInteger, StructuralBounds, TerminationWitness,
    };
    use serde_json::json;

    use super::{VerificationError, finalize_verified_with_invariant_probe};

    #[test]
    fn post_validation_invariant_failure_is_internal() {
        let graph: GraphSpec = serde_json::from_value(json!({
            "profile":"openengine.graph.full/v1",
            "initialInput":{"kind":"null"},
            "policy":{"policy":"policy.strict@1","default":"deny"},
            "root":{
                "kind":"seq","name":"duplicate","state":{"kind":"null"},
                "children":[
                    {"kind":"step","name":"duplicate","worker":"worker@1","input":{"kind":"null"},"output":{"kind":"null"},"inputBindings":[],"writeBindings":[],"timeoutMs":1,"attempts":1},
                    {"kind":"succeed","name":"done","output":{"kind":"null"},"bindings":[]}
                ],
                "promotedStatePaths":[]
            }
        }))
        .unwrap();
        let one = PositiveInteger::new(1).unwrap();
        let bounds = StructuralBounds {
            termination: TerminationWitness::Acyclic {
                order: NonEmptyVec::new(vec![NodeName::new("duplicate").unwrap()]).unwrap(),
            },
            max_node_executions: one,
            peak_concurrency: one,
            attempts_per_node: BTreeMap::from([(NodeName::new("duplicate").unwrap(), one)]),
        };

        assert_eq!(
            finalize_verified_with_invariant_probe(&graph, bounds, true),
            Err(VerificationError::Internal(
                "injected post-validation invariant failure".to_owned()
            ))
        );
    }
}
