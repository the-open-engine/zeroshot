use super::*;

impl Analyzer<'_> {
    pub(super) fn fold_node(&mut self, node: &GraphNode) -> Option<Fold> {
        let path = self.node_path(node.name());
        match node {
            GraphNode::Step(_) | GraphNode::Verifier(_) => Some(Fold {
                executions: 1,
                concurrency: 1,
                ..Fold::default()
            }),
            GraphNode::Succeed(_) | GraphNode::Fail(_) => Some(Fold::default()),
            GraphNode::Seq(group) => self.fold_children(group.children.as_slice(), false, &path),
            GraphNode::Par(group) => self.fold_children(group.branches.as_slice(), true, &path),
            GraphNode::Choice(group) => self.fold_choice(group),
            GraphNode::Map(group) => self.fold_map(group),
            GraphNode::Loop(group) => self.fold_loop(group, &path),
        }
    }

    fn fold_choice(&mut self, group: &ChoiceNode) -> Option<Fold> {
        let children = group
            .branches
            .as_slice()
            .iter()
            .map(|branch| &branch.node)
            .chain(group.otherwise.as_deref());
        let mut result = Fold::default();
        for child in children {
            let fold = self.fold_node(child)?;
            result.executions = result.executions.max(fold.executions);
            result.concurrency = result.concurrency.max(fold.concurrency);
            result.loop_entries = result.loop_entries.max(fold.loop_entries);
            result.dynamic |= fold.dynamic;
        }
        Some(result)
    }

    fn fold_map(&mut self, group: &MapNode) -> Option<Fold> {
        let body = self.fold_node(&group.body)?;
        Some(Fold {
            executions: if body.executions == 0 {
                0
            } else {
                FULL_V1_MAX_NODE_EXECUTIONS
            },
            loop_entries: if body.loop_entries == 0 {
                0
            } else {
                FULL_V1_MAX_LOOP_ENTRIES
            },
            concurrency: group
                .max_items
                .get()
                .saturating_mul(body.concurrency)
                .min(FULL_V1_MAX_PEAK_CONCURRENCY),
            dynamic: true,
        })
    }

    fn fold_loop(
        &mut self,
        group: &openengine_cluster_protocol::LoopNode,
        path: &[DiagnosticPathSegment],
    ) -> Option<Fold> {
        let body = self.fold_node(&group.body)?;
        let path = with_field(path, "maxIterations");
        Some(Fold {
            executions: self.work_bound(
                (
                    body.executions.saturating_mul(group.max_iterations.get()),
                    body.dynamic,
                ),
                FULL_V1_MAX_NODE_EXECUTIONS,
                &path,
            )?,
            loop_entries: self.work_bound(
                (
                    body.loop_entries
                        .saturating_add(1)
                        .saturating_mul(group.max_iterations.get()),
                    body.dynamic,
                ),
                FULL_V1_MAX_LOOP_ENTRIES,
                &path,
            )?,
            concurrency: body.concurrency,
            dynamic: body.dynamic,
        })
    }

    fn fold_children(
        &mut self,
        children: &[GraphNode],
        parallel: bool,
        path: &[DiagnosticPathSegment],
    ) -> Option<Fold> {
        let mut folds = Vec::new();
        for child in children {
            folds.push(self.fold_node(child)?);
        }
        let dynamic = folds.iter().any(|fold| fold.dynamic);
        Some(Fold {
            executions: self.work_bound(
                (
                    folds
                        .iter()
                        .fold(0_u64, |total, fold| total.saturating_add(fold.executions)),
                    dynamic,
                ),
                FULL_V1_MAX_NODE_EXECUTIONS,
                path,
            )?,
            loop_entries: self.work_bound(
                (
                    folds
                        .iter()
                        .fold(0_u64, |total, fold| total.saturating_add(fold.loop_entries)),
                    dynamic,
                ),
                FULL_V1_MAX_LOOP_ENTRIES,
                path,
            )?,
            concurrency: if parallel {
                folds
                    .iter()
                    .fold(0_u64, |total, fold| total.saturating_add(fold.concurrency))
                    .min(FULL_V1_MAX_PEAK_CONCURRENCY)
            } else {
                folds.iter().map(|fold| fold.concurrency).max().unwrap_or(0)
            },
            dynamic,
        })
    }

    fn work_bound(
        &mut self,
        work: (u64, bool),
        bound: u64,
        path: &[DiagnosticPathSegment],
    ) -> Option<u64> {
        let (actual, dynamic) = work;
        if actual <= bound || dynamic {
            return Some(actual.min(bound));
        }
        emit_diagnostic!(
            self,
            GraphDiagnosticCode::CeilingExceeded,
            format!("static work exceeds run ceiling {bound}"),
            path.to_vec(),
            Vec::new()
        );
        None
    }
}
