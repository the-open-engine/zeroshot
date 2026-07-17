use super::*;

impl<'a> Analyzer<'a> {
    pub(super) fn validate_executable(
        &mut self,
        context: ExecutableValidationContext<'_>,
    ) -> Effects {
        self.validate_input_bindings(
            context.input_bindings,
            InputBindingsValidationContext {
                target_payload: context.input,
                node: context.node,
                node_path: context.path,
                field: "inputBindings",
                target_field: "input",
            },
        );
        let writes = self.validate_write_bindings(
            context.write_bindings,
            &WriteBindingsValidationContext {
                name: context.name,
                output: context.output,
                incoming: context.node.incoming,
                state: context.node.state,
                map_index_targets: context.node.map_index_targets,
                path: context.path,
            },
        );
        let outcome_writes = if writes.is_empty() {
            BTreeMap::new()
        } else {
            BTreeMap::from([(context.name.clone(), writes.clone())])
        };
        let outcome_order = if outcome_writes.is_empty() {
            Vec::new()
        } else {
            vec![context.name.clone()]
        };
        let possible_write_types = writes
            .iter()
            .map(|(path, write)| (path.clone(), vec![write.value_type.clone()]))
            .collect();
        Effects {
            definite_nodes: BTreeSet::from([context.name.clone()]),
            possible_writes: writes,
            possible_write_types,
            outcome_writes,
            outcome_order,
            exit_failed: context
                .node
                .incoming
                .failed
                .union(&BTreeSet::from([context.name.clone()]))
                .cloned()
                .collect(),
            falls_through: true,
            completion: CompletionPredicate::Always,
            ..Effects::default()
        }
    }

    pub(super) fn validate_write_bindings(
        &mut self,
        bindings: &[WriteBinding],
        context: &WriteBindingsValidationContext<'_>,
    ) -> Writes {
        let mut writes = Writes::new();
        let mut targets = Vec::<FieldPath>::new();
        for (index, binding) in bindings.iter().enumerate() {
            let binding_path = indexed_field_path(context.path, "writeBindings", index);
            let binding_context = WriteBindingsValidationContext {
                path: &binding_path,
                ..*context
            };
            if let Some(producer) = self.validate_write_binding(binding, &binding_context) {
                writes.insert(binding.target.clone(), producer);
            }
            self.validate_target_overlap(
                &binding.target,
                TargetOverlapValidationContext {
                    previous: &targets,
                    path: &field_path(&binding_path, &["target"]),
                    message: "executable has overlapping write targets",
                    related_nodes: vec![context.name.clone()],
                },
            );
            targets.push(binding.target.clone());
        }
        writes
    }

    pub(super) fn validate_write_binding(
        &mut self,
        binding: &WriteBinding,
        context: &WriteBindingsValidationContext<'_>,
    ) -> Option<WriteFact> {
        let declared_target = path_type(context.state, &binding.target);
        if declared_target.is_none() {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::SchemaSafety,
                "write target does not exist in node state",
                with_field(context.path, "target"),
                vec![context.name.clone()],
            );
        }
        let target = context
            .map_index_targets
            .and_then(|targets| targets.get(&binding.target))
            .or(declared_target);
        let producer = self.validate_output_selector(
            &binding.value,
            &OutputSelectorValidationContext {
                current: context.name,
                incoming: context.incoming,
                current_output: context.output,
                path: context.path,
            },
        );
        if let (Some(producer), Some(target)) = (&producer, target) {
            if !producer.value_type.is_subtype_of(target) {
                emit_diagnostic!(
                    self,
                    GraphDiagnosticCode::SchemaSafety,
                    "write value is not a subtype of its state target",
                    context.path.to_vec(),
                    vec![binding.value.node.clone(), context.name.clone()],
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

    pub(super) fn validate_input_bindings(
        &mut self,
        bindings: &[InputBinding],
        context: InputBindingsValidationContext<'_>,
    ) {
        if !matches!(
            context.target_payload,
            PayloadType::Null | PayloadType::Record { .. }
        ) {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::SchemaSafety,
                "v1 field bindings cannot construct a whole non-record payload; use null or record",
                with_field(context.node_path, context.target_field),
                Vec::new(),
            );
        }
        let mut targets = Vec::<FieldPath>::new();
        for (index, binding) in bindings.iter().enumerate() {
            let binding_path = indexed_field_path(context.node_path, context.field, index);
            let binding_context = InputBindingsValidationContext {
                node_path: &binding_path,
                ..context
            };
            self.validate_input_binding(binding, &binding_context);
            self.validate_target_overlap(
                &binding.target,
                TargetOverlapValidationContext {
                    previous: &targets,
                    path: &with_field(&binding_path, "target"),
                    message: "bindings have overlapping targets",
                    related_nodes: Vec::new(),
                },
            );
            targets.push(binding.target.clone());
        }
        self.validate_required_binding_targets(&targets, &context);
    }

    pub(super) fn validate_input_binding(
        &mut self,
        binding: &InputBinding,
        context: &InputBindingsValidationContext<'_>,
    ) {
        let target = path_type(context.target_payload, &binding.target);
        if target.is_none() {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::SchemaSafety,
                "binding target does not exist in declared payload",
                with_field(context.node_path, "target"),
                Vec::new(),
            );
        }
        let source = self.validate_data_selector(
            &binding.value,
            context.node,
            &with_field(context.node_path, "value"),
        );
        if let (Some(source), Some(target)) = (source, target) {
            if !source.is_subtype_of(target) {
                emit_diagnostic!(
                    self,
                    GraphDiagnosticCode::SchemaSafety,
                    "binding source is not a subtype of its target",
                    context.node_path.to_vec(),
                    Vec::new(),
                );
            }
        }
    }

    pub(super) fn validate_required_binding_targets(
        &mut self,
        targets: &[FieldPath],
        context: &InputBindingsValidationContext<'_>,
    ) {
        for required in required_leaf_paths(context.target_payload) {
            if !targets
                .iter()
                .any(|target| path_is_prefix(target, &required))
            {
                emit_diagnostic!(
                    self,
                    GraphDiagnosticCode::UndefinedRead,
                    format!(
                        "required payload target {} is not defined by a binding",
                        display_field_path(&required)
                    ),
                    with_field(context.node_path, context.field),
                    Vec::new(),
                );
            }
        }
    }

    pub(super) fn validate_target_overlap(
        &mut self,
        target: &FieldPath,
        context: TargetOverlapValidationContext<'_>,
    ) {
        if context
            .previous
            .iter()
            .any(|other| paths_overlap(other, target))
        {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::WriteConflict,
                context.message,
                context.path.to_vec(),
                context.related_nodes,
            );
        }
    }

    pub(super) fn validate_data_selector<'b>(
        &mut self,
        selector: &DataSelector,
        context: NodeValidationContext<'b>,
        path: &[DiagnosticPathSegment],
    ) -> Option<PayloadType> {
        let (payload, selected, selected_override, indexed_definition, defined) = match selector {
            DataSelector::State { path: selected } => {
                let selected_override = context
                    .map_index_targets
                    .and_then(|targets| targets.get(selected));
                let indexed_definition = selected_override.and_then(|expected| {
                    context
                        .incoming
                        .defined
                        .get(selected)
                        .filter(|actual| compatible_types(actual, expected))
                });
                (
                    Some(context.state),
                    selected,
                    selected_override,
                    indexed_definition,
                    if selected_override.is_some() {
                        indexed_definition.is_some()
                    } else {
                        is_required_path(context.state, selected)
                            || is_defined(&context.incoming.defined, selected)
                    },
                )
            }
            DataSelector::Item { path: selected } => (
                context.item,
                selected,
                None,
                None,
                context
                    .item
                    .is_some_and(|payload| is_required_path(payload, selected)),
            ),
        };
        let Some(payload) = payload else {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::InvalidGraphShape,
                "item selector is legal only inside a map body",
                path.to_vec(),
                Vec::new(),
            );
            return None;
        };
        let selected_type = selected_override.or_else(|| path_type(payload, selected));
        if selected_type.is_none() {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::SchemaSafety,
                "selector path does not exist in its payload type",
                path.to_vec(),
                Vec::new(),
            );
        } else if !defined {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::UndefinedRead,
                "selector path is not definitely defined on every reaching path",
                path.to_vec(),
                Vec::new(),
            );
        }
        match selector {
            DataSelector::State { .. } => {
                if selected_override.is_some() {
                    indexed_definition
                        .cloned()
                        .or_else(|| selected_type.cloned())
                } else {
                    context
                        .incoming
                        .defined
                        .get(selected)
                        .cloned()
                        .or_else(|| selected_type.cloned())
                }
            }
            DataSelector::Item { .. } => selected_type.cloned(),
        }
    }

    pub(super) fn validate_output_selector(
        &mut self,
        selector: &NodeOutputSelector,
        context: &OutputSelectorValidationContext<'_>,
    ) -> Option<SelectedOutput> {
        self.validate_output_availability(selector, context);
        let selected = self.output_selector(selector, context.current, context.current_output);
        if selected.is_none() {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::SchemaSafety,
                "node output selector channel or path is invalid",
                with_field(context.path, "value"),
                vec![selector.node.clone()],
            );
        }
        selected
    }

    pub(super) fn validate_output_availability(
        &mut self,
        selector: &NodeOutputSelector,
        context: &OutputSelectorValidationContext<'_>,
    ) {
        if selector.node != *context.current
            && (!context.incoming.available.contains(&selector.node)
                || context.incoming.unavailable.contains(&selector.node))
        {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::UndefinedRead,
                format!("node output {} does not dominate this read", selector.node),
                field_path(context.path, &["value"]),
                vec![selector.node.clone(), context.current.clone()],
            );
        }
        if context.incoming.failed.contains(&selector.node)
            && matches!(
                selector.channel,
                NodeOutputChannel::Out | NodeOutputChannel::Signal | NodeOutputChannel::Diagnostic
            )
        {
            emit_diagnostic!(
                self,
                GraphDiagnosticCode::UndefinedRead,
                format!(
                    "node output {} is unavailable on its error path",
                    selector.node
                ),
                field_path(context.path, &["value"]),
                vec![selector.node.clone(), context.current.clone()],
            );
        }
    }

    pub(super) fn output_selector(
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

    pub(super) fn selector_verifier(&self, name: &NodeName) -> Option<&VerifierNode> {
        match self.nodes.get(name).map(|info| info.node) {
            Some(GraphNode::Verifier(verifier)) => Some(verifier),
            _ => None,
        }
    }

    pub(super) fn validate_map_selector(
        &mut self,
        map: &MapNode,
        context: &LocatedNodeValidationContext<'_>,
    ) -> Option<PayloadType> {
        let selected = self.validate_data_selector(
            &map.over,
            NodeValidationContext {
                state: &map.state,
                ..context.node
            },
            &with_field(&context.path, "over"),
        );
        match selected {
            Some(PayloadType::Array { items }) => Some((*items).clone()),
            Some(_) => {
                emit_diagnostic!(
                    self,
                    GraphDiagnosticCode::SchemaSafety,
                    "map selector must resolve to an array",
                    with_field(&context.path, "over"),
                    vec![map.name.clone()],
                );
                None
            }
            None => None,
        }
    }
}
