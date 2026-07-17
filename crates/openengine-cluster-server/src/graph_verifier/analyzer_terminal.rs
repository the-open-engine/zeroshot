use super::*;

impl<'a> Analyzer<'a> {
    pub(super) fn validate_terminal_coverage(&mut self) {
        self.terminal_flow(&self.graph.root);
        if self.may_fall_through(&self.graph.root) {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::Reachability,
                "normal-success path falls through without explicit succeed or fail terminal",
                vec![field_segment("root")],
                vec![self.graph.root.name().clone()],
            );
        }
    }

    pub(super) fn terminal_flow(&mut self, node: &GraphNode) {
        match node {
            GraphNode::Seq(group) => self.terminal_sequence(group),
            GraphNode::Choice(group) => self.terminal_choice(group),
            GraphNode::Par(group) => self.terminal_parallel(group),
            GraphNode::Loop(group) => self.terminal_flow(&group.body),
            GraphNode::Map(group) => self.terminal_flow(&group.body),
            GraphNode::Step(_)
            | GraphNode::Verifier(_)
            | GraphNode::Succeed(_)
            | GraphNode::Fail(_) => {}
        }
    }

    pub(super) fn terminal_sequence(&mut self, group: &SeqNode) {
        let mut reachable = true;
        for child in group.children.as_slice() {
            if !reachable {
                emit_diagnostic!(
                    self,
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

    pub(super) fn terminal_choice(&mut self, group: &ChoiceNode) {
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

    pub(super) fn terminal_parallel(&mut self, group: &ParNode) {
        for branch in group.branches.as_slice() {
            self.terminal_flow(branch);
        }
    }

    pub(super) fn may_fall_through(&self, node: &GraphNode) -> bool {
        self.node_fallthrough
            .get(&node_identity(node))
            .copied()
            .unwrap_or(true)
    }

    pub(super) fn reference_topological_order(&mut self) -> Vec<NodeName> {
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

    pub(super) fn validate_reference_names(&mut self, known: &BTreeSet<NodeName>) {
        for (node, dependencies) in self.dependencies.clone() {
            for dependency in dependencies {
                if !known.contains(&dependency) {
                    emit_diagnostic!(
                        self,
                        GraphDiagnosticCode::UndefinedRead,
                        format!("reference names unknown node {dependency}"),
                        self.node_path(&node),
                        vec![dependency],
                    );
                }
            }
        }
    }

    pub(super) fn next_reference_node(
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

    pub(super) fn emit_reference_cycle(&mut self, remaining: &BTreeSet<NodeName>) {
        let related = remaining.iter().cloned().collect::<Vec<_>>();
        let path = related
            .first()
            .map_or_else(|| vec![field_segment("root")], |name| self.node_path(name));
        emit_diagnostic!(
            self,
            GraphDiagnosticCode::CyclicReference,
            "node-output/control references contain a cycle",
            path,
            related,
        );
    }

    pub(super) fn worker_diagnostic(
        &self,
        diagnostic: WorkerCompatibilityDiagnostic,
    ) -> GraphDiagnostic {
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

    pub(super) fn node_path(&self, name: &NodeName) -> Vec<DiagnosticPathSegment> {
        self.nodes
            .get(name)
            .map_or_else(|| vec![field_segment("root")], |info| info.path.clone())
    }

    pub(super) fn emit(&mut self, mut diagnostic: DiagnosticDetails) {
        diagnostic.related_nodes.sort();
        diagnostic.related_nodes.dedup();
        self.diagnostics.push(GraphDiagnostic {
            severity: DiagnosticSeverity::Error,
            code: diagnostic.code,
            message: diagnostic.message,
            path: diagnostic.path,
            related_nodes: diagnostic.related_nodes,
        });
    }

    pub(super) fn sort_diagnostics(&mut self) {
        sort_diagnostics(&mut self.diagnostics);
    }
}
