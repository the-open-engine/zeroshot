//! References to files exposed at explicit human requests and outer graph success.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Managed references retain captured bytes. Local references resolve the current working file.
/// Neither form contains file bytes or changes the graph's authored JSON result.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublishedFile {
    Managed {
        target: String,
        address: crate::FileAddress,
    },
    Local {
        root: String,
        path: String,
    },
}

pub type PublishedFiles = std::collections::BTreeMap<crate::FieldName, PublishedFile>;
