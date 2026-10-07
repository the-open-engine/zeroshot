fn audit_probe_state() -> Result<PayloadType, BuiltinTemplateError> {
    record_type(vec![
        (TASK_FIELD, PayloadType::String, true),
        (WORK_ITEMS_FIELD, array_type(PayloadType::Null), true),
        (CONTINUATION_ITEMS_FIELD, array_type(PayloadType::Null), true),
    ])
}

fn probe_graph(
    state: PayloadType,
    name: &str,
    mut children: Vec<GraphNode>,
    done: &str,
) -> Result<GraphSpec, BuiltinTemplateError> {
    children.push(succeed_null(done)?);
    graph(
        state.clone(),
        sequence(name, state, children, Vec::new())?,
    )
}

pub(super) fn audit_probe_graph() -> Result<GraphSpec, BuiltinTemplateError> {
    let state = audit_probe_state()?;
    probe_graph(
        state.clone(),
        "audit_probe",
        vec![
            disposition_audit_phase(state.clone())?,
            exit_route(state, TemplateDelivery::None)?,
        ],
        "audit_probe_done",
    )
}

pub(super) fn audit_loop_probe_graph() -> Result<GraphSpec, BuiltinTemplateError> {
    let state = audit_probe_state()?;
    let iteration = sequence(
        "audit_loop_probe_iteration",
        state.clone(),
        vec![
            disposition_audit_phase(state.clone())?,
            exit_route(state.clone(), TemplateDelivery::None)?,
        ],
        Vec::new(),
    )?;
    probe_graph(
        state.clone(),
        "audit_loop_probe",
        vec![GraphNode::Loop(LoopNode {
            name: node_name("audit_loop_probe_loop")?,
            state,
            body: Box::new(iteration),
            until: None,
            max_iterations: positive(3)?,
            max_iterations_input: None,
            promoted_state_paths: Vec::new(),
        })],
        "audit_loop_probe_done",
    )
}

pub(super) fn experiment_failure_probe_graph() -> Result<GraphSpec, BuiltinTemplateError> {
    let state = record_type(vec![
        (TASK_FIELD, PayloadType::String, true),
        (WORK_ITEMS_FIELD, role_array_type(&WORK_ITEM_LABELS)?, true),
        (JUDGE_ROLES_FIELD, role_array_type(&JUDGE_ROLE_LABELS)?, true),
        (REVIEWS_FIELD, array_type(PayloadType::String), true),
        (VERDICTS_FIELD, array_type(verdict_type()?), true),
        (TITLE_FIELD, PayloadType::String, true),
        (DESCRIPTION_FIELD, PayloadType::String, true),
    ])?;
    probe_graph(
        state.clone(),
        "experiment_failure_probe",
        vec![experiment_stage(state)?],
        "experiment_probe_done",
    )
}

pub(super) fn recorder_audit_probe_graph() -> Result<GraphSpec, BuiltinTemplateError> {
    let PayloadType::Record { mut fields } = audit_probe_state()? else {
        unreachable!("audit probe state is a record")
    };
    fields.insert(field_name(REVIEWS_FIELD)?, required(array_type(PayloadType::String)));
    fields.insert(field_name(VERDICTS_FIELD)?, required(array_type(verdict_type()?)));
    fields.insert(field_name(TITLE_FIELD)?, required(PayloadType::String));
    fields.insert(field_name(DESCRIPTION_FIELD)?, required(PayloadType::String));
    let state = PayloadType::Record { fields };
    probe_graph(
        state.clone(),
        "recorder_audit_probe",
        vec![
            stop_stage(state.clone())?,
            disposition_audit_phase(state.clone())?,
            exit_route(state, TemplateDelivery::None)?,
        ],
        "recorder_audit_probe_done",
    )
}
