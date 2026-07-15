//! Structured future-verifier diagnostics and structural bound witnesses.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{FieldName, FieldPath, NodeName, NonEmptyVec, PositiveInteger};

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphDiagnosticCode {
    SchemaSafety,
    Reachability,
    ChoiceExhaustiveness,
    LoopExitSatisfiability,
    MissingBound,
    WriteConflict,
    CeilingExceeded,
    CyclicReference,
    UndefinedRead,
    InvalidGraphShape,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "snake_case")]
pub enum DiagnosticPathSegment {
    Field {
        name: FieldName,
    },
    Index {
        #[serde(deserialize_with = "crate::value::deserialize_u32")]
        #[schemars(range(max = 4_294_967_295_u64))]
        index: u32,
    },
    Node {
        name: NodeName,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct GraphDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: GraphDiagnosticCode,
    pub message: String,
    pub path: Vec<DiagnosticPathSegment>,
    pub related_nodes: Vec<NodeName>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    deny_unknown_fields,
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum TerminationWitness {
    Acyclic {
        order: NonEmptyVec<NodeName>,
    },
    Bounded {
        ranking: NonEmptyVec<FieldPath>,
        max_iterations: PositiveInteger,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StructuralBounds {
    pub termination: TerminationWitness,
    pub max_node_executions: PositiveInteger,
    pub peak_concurrency: PositiveInteger,
    #[schemars(
        schema_with = "crate::value::identifier_keyed_map_schema::<NodeName, PositiveInteger>"
    )]
    pub attempts_per_node: BTreeMap<NodeName, PositiveInteger>,
}
