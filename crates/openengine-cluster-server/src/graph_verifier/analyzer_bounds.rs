use super::*;

impl<'a> Analyzer<'a> {
    pub(super) fn fold_node(&mut self, node: &GraphNode) -> Option<Fold> {
        let path = self.node_path(node.name());
        let fold = match node {
            GraphNode::Step(_) | GraphNode::Verifier(_) => Fold {
                executions: 1,
                concurrency: 1,
                loop_entries: 0,
            },
            GraphNode::Succeed(_) | GraphNode::Fail(_) => Fold::default(),
            _ => self.fold_group_node(node, &path)?,
        };
        self.check_fold_limits(fold, &path)
    }

    pub(super) fn fold_group_node(
        &mut self,
        node: &GraphNode,
        path: &[DiagnosticPathSegment],
    ) -> Option<Fold> {
        match node {
            GraphNode::Seq(group) => self.fold_seq(group, path),
            GraphNode::Choice(group) => self.fold_choice(group),
            GraphNode::Par(group) => self.fold_par(group, path),
            GraphNode::Map(group) => self.fold_map(group, path),
            GraphNode::Loop(group) => self.fold_loop(group, path),
            GraphNode::Step(_)
            | GraphNode::Verifier(_)
            | GraphNode::Succeed(_)
            | GraphNode::Fail(_) => None,
        }
    }

    pub(super) fn fold_seq(
        &mut self,
        group: &SeqNode,
        path: &[DiagnosticPathSegment],
    ) -> Option<Fold> {
        let children = group
            .children
            .as_slice()
            .iter()
            .filter_map(|child| self.fold_node(child))
            .collect::<Vec<_>>();
        Some(Fold {
            executions: self.checked_sum(
                children.iter().map(|fold| fold.executions),
                FoldLimitContext {
                    ceiling: FULL_V1_MAX_NODE_EXECUTIONS,
                    path,
                    field: "children",
                    label: "node executions",
                },
            )?,
            concurrency: children
                .iter()
                .map(|fold| fold.concurrency)
                .max()
                .unwrap_or(0),
            loop_entries: self.checked_sum(
                children.iter().map(|fold| fold.loop_entries),
                FoldLimitContext {
                    ceiling: FULL_V1_MAX_LOOP_ENTRIES,
                    path,
                    field: "children",
                    label: "loop entries",
                },
            )?,
        })
    }

    pub(super) fn fold_choice(&mut self, group: &ChoiceNode) -> Option<Fold> {
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

    pub(super) fn fold_par(
        &mut self,
        group: &ParNode,
        path: &[DiagnosticPathSegment],
    ) -> Option<Fold> {
        let children = group
            .branches
            .as_slice()
            .iter()
            .filter_map(|branch| self.fold_node(branch))
            .collect::<Vec<_>>();
        Some(Fold {
            executions: self.checked_sum(
                children.iter().map(|fold| fold.executions),
                FoldLimitContext {
                    ceiling: FULL_V1_MAX_NODE_EXECUTIONS,
                    path,
                    field: "branches",
                    label: "node executions",
                },
            )?,
            concurrency: self.checked_sum(
                children.iter().map(|fold| fold.concurrency),
                FoldLimitContext {
                    ceiling: FULL_V1_MAX_PEAK_CONCURRENCY,
                    path,
                    field: "branches",
                    label: "peak concurrency",
                },
            )?,
            loop_entries: self.checked_sum(
                children.iter().map(|fold| fold.loop_entries),
                FoldLimitContext {
                    ceiling: FULL_V1_MAX_LOOP_ENTRIES,
                    path,
                    field: "branches",
                    label: "loop entries",
                },
            )?,
        })
    }

    pub(super) fn fold_map(
        &mut self,
        group: &MapNode,
        path: &[DiagnosticPathSegment],
    ) -> Option<Fold> {
        let body = self.fold_node(&group.body)?;
        Some(Fold {
            executions: self.checked_product(
                group.max_items.get(),
                body.executions,
                FoldLimitContext {
                    ceiling: FULL_V1_MAX_NODE_EXECUTIONS,
                    path,
                    field: "maxItems",
                    label: "node executions",
                },
            )?,
            concurrency: self.checked_product(
                group.max_items.get(),
                body.concurrency,
                FoldLimitContext {
                    ceiling: FULL_V1_MAX_PEAK_CONCURRENCY,
                    path,
                    field: "maxItems",
                    label: "peak concurrency",
                },
            )?,
            loop_entries: self.checked_product(
                group.max_items.get(),
                body.loop_entries,
                FoldLimitContext {
                    ceiling: FULL_V1_MAX_LOOP_ENTRIES,
                    path,
                    field: "maxItems",
                    label: "loop entries",
                },
            )?,
        })
    }

    pub(super) fn fold_loop(
        &mut self,
        group: &LoopNode,
        path: &[DiagnosticPathSegment],
    ) -> Option<Fold> {
        let body = self.fold_node(&group.body)?;
        let entries = body.loop_entries.checked_add(1).or_else(|| {
            self.ceiling(path, "maxIterations", "loop-entry arithmetic overflow");
            None
        })?;
        Some(Fold {
            executions: self.checked_product(
                group.max_iterations.get(),
                body.executions,
                FoldLimitContext {
                    ceiling: FULL_V1_MAX_NODE_EXECUTIONS,
                    path,
                    field: "maxIterations",
                    label: "node executions",
                },
            )?,
            concurrency: body.concurrency,
            loop_entries: self.checked_product(
                group.max_iterations.get(),
                entries,
                FoldLimitContext {
                    ceiling: FULL_V1_MAX_LOOP_ENTRIES,
                    path,
                    field: "maxIterations",
                    label: "loop entries",
                },
            )?,
        })
    }

    pub(super) fn check_fold_limits(
        &mut self,
        fold: Fold,
        path: &[DiagnosticPathSegment],
    ) -> Option<Fold> {
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

    pub(super) fn checked_sum(
        &mut self,
        values: impl Iterator<Item = u64>,
        context: FoldLimitContext<'_>,
    ) -> Option<u64> {
        let mut total = 0_u64;
        for value in values {
            total = match total.checked_add(value) {
                Some(total) if total <= context.ceiling => total,
                _ => {
                    self.ceiling(
                        context.path,
                        context.field,
                        format!("{} exceed {}", context.label, context.ceiling),
                    );
                    return None;
                }
            };
        }
        Some(total)
    }

    pub(super) fn checked_product(
        &mut self,
        left: u64,
        right: u64,
        context: FoldLimitContext<'_>,
    ) -> Option<u64> {
        match left.checked_mul(right) {
            Some(product) if product <= context.ceiling => Some(product),
            _ => {
                self.ceiling(
                    context.path,
                    context.field,
                    format!("{} exceed {}", context.label, context.ceiling),
                );
                None
            }
        }
    }

    pub(super) fn ceiling(
        &mut self,
        path: &[DiagnosticPathSegment],
        field: &str,
        message: impl Into<String>,
    ) {
        emit_diagnostic!(
            self,
            GraphDiagnosticCode::CeilingExceeded,
            message,
            with_field(path, field),
            Vec::new(),
        );
    }
}
