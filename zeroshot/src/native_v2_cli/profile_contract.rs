use openengine_cluster_protocol::{
    RunProfileListResult, RunProfileName, RunProfileScope, RunProfileSummary,
};
use serde::Serialize;

use super::{NativeV2CliError, RunGraph, RunRuntime};

/// Local CLI/UI listing diagnostics; the target's v1 wire result remains unchanged.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliProfileListResult {
    pub profiles: Vec<RunProfileSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<CliProfileProblem>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliProfileProblem {
    pub name: RunProfileName,
    pub scope: RunProfileScope,
    pub is_default: bool,
    pub message: String,
}

impl From<RunProfileListResult> for CliProfileListResult {
    fn from(value: RunProfileListResult) -> Self {
        Self {
            profiles: value.profiles,
            problems: Vec::new(),
        }
    }
}

impl CliProfileListResult {
    /// Legacy callers cannot represent partial listings, so retain their explicit failure.
    pub(crate) fn into_protocol(self) -> Result<RunProfileListResult, NativeV2CliError> {
        if let Some(problem) = self.problems.into_iter().next() {
            return Err(NativeV2CliError::Local(problem.message));
        }
        Ok(RunProfileListResult {
            profiles: self.profiles,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunSelection {
    Inline {
        graph: RunGraph,
        runtime: RunRuntime,
    },
    /// `None` means resolve the first available scoped default.
    Profile(Option<ProfileReference>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileQualifier {
    Local,
    User,
    Org,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileReference {
    pub qualifier: Option<ProfileQualifier>,
    pub name: RunProfileName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileRoute {
    pub target: Option<String>,
    pub scope: Option<RunProfileScope>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileSetCommand {
    pub route: ProfileRoute,
    pub name: RunProfileName,
    pub graph: RunGraph,
    pub runtime: RunRuntime,
    pub set_default: bool,
}
