//! Filepath values supplied at admission. Bytes use the independent file transfer service.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A filepath input selects target bytes or a private upload. Hosts replace the selection with
/// a run-owned path before graph execution; selectors never authorize arbitrary host paths.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunFileInput {
    Target {
        selection: crate::FileSelection,
    },
    Staged {
        #[serde(rename = "runId")]
        run_id: crate::RunId,
        operation: crate::IdempotencyKey,
    },
}
