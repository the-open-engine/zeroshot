use super::*;

#[derive(Clone, Copy)]
pub(super) struct NodeValidationContext<'a> {
    pub(super) incoming: &'a Flow,
    pub(super) state: &'a PayloadType,
    pub(super) item: Option<&'a PayloadType>,
}

pub(super) struct LocatedNodeValidationContext<'a> {
    pub(super) node: NodeValidationContext<'a>,
    pub(super) path: Vec<DiagnosticPathSegment>,
}

#[derive(Clone, Copy)]
pub(super) struct ExecutableValidationContext<'a> {
    pub(super) name: &'a NodeName,
    pub(super) input: &'a PayloadType,
    pub(super) output: &'a PayloadType,
    pub(super) input_bindings: &'a [InputBinding],
    pub(super) write_bindings: &'a [WriteBinding],
    pub(super) node: NodeValidationContext<'a>,
    pub(super) path: &'a [DiagnosticPathSegment],
}

#[derive(Clone, Copy)]
pub(super) struct InputBindingsValidationContext<'a> {
    pub(super) target_payload: &'a PayloadType,
    pub(super) node: NodeValidationContext<'a>,
    pub(super) node_path: &'a [DiagnosticPathSegment],
    pub(super) field: &'a str,
}

#[derive(Clone, Copy)]
pub(super) struct WriteBindingsValidationContext<'a> {
    pub(super) name: &'a NodeName,
    pub(super) output: &'a PayloadType,
    pub(super) incoming: &'a Flow,
    pub(super) state: &'a PayloadType,
    pub(super) path: &'a [DiagnosticPathSegment],
}

#[derive(Clone, Copy)]
pub(super) struct OutputSelectorValidationContext<'a> {
    pub(super) current: &'a NodeName,
    pub(super) incoming: &'a Flow,
    pub(super) current_output: &'a PayloadType,
    pub(super) path: &'a [DiagnosticPathSegment],
}

pub(super) struct TargetOverlapValidationContext<'a> {
    pub(super) previous: &'a [FieldPath],
    pub(super) path: &'a [DiagnosticPathSegment],
    pub(super) message: &'a str,
    pub(super) related_nodes: Vec<NodeName>,
}

pub(super) struct ExecutableIndexContext<'a> {
    pub(super) name: &'a NodeName,
    pub(super) attempts: PositiveInteger,
    pub(super) path: &'a [DiagnosticPathSegment],
    pub(super) bindings: &'a [WriteBinding],
}

#[derive(Clone, Copy)]
pub(super) struct GuardValidationContext<'a> {
    pub(super) path: &'a [DiagnosticPathSegment],
    pub(super) code: GraphDiagnosticCode,
}

pub(super) struct SelectorLabelsValidationContext<'a> {
    pub(super) labels: &'a [EnumLabel],
    pub(super) guard: GuardValidationContext<'a>,
    pub(super) message: &'a str,
}

pub(super) struct KOfNLabelsValidationContext<'a> {
    pub(super) count: u64,
    pub(super) selectors: &'a [ControlSelector],
    pub(super) labels: &'a [EnumLabel],
    pub(super) guard: GuardValidationContext<'a>,
}

pub(super) struct KOfMapLabelsValidationContext<'a> {
    pub(super) count: u64,
    pub(super) selector: &'a ControlSelector,
    pub(super) labels: &'a [EnumLabel],
    pub(super) guard: GuardValidationContext<'a>,
}

#[derive(Clone, Copy)]
pub(super) struct FoldLimitContext<'a> {
    pub(super) ceiling: u64,
    pub(super) path: &'a [DiagnosticPathSegment],
    pub(super) field: &'a str,
    pub(super) label: &'a str,
}

pub(super) struct PromotionValidationContext<'a> {
    pub(super) group_state: &'a PayloadType,
    pub(super) enclosing_state: &'a PayloadType,
    pub(super) promoted: &'a [FieldPath],
    pub(super) effects: &'a mut Effects,
    pub(super) path: &'a [DiagnosticPathSegment],
    pub(super) rule: PromotionRule,
}

pub(super) struct DiagnosticDetails {
    pub(super) code: GraphDiagnosticCode,
    pub(super) message: String,
    pub(super) path: Vec<DiagnosticPathSegment>,
    pub(super) related_nodes: Vec<NodeName>,
}
