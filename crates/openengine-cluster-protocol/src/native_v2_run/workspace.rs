//! Explicit source-free execution. Omission retains repository mode.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const RUN_WORKSPACES_KIND: &str = "openengine.run-workspaces/v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePlacement {
    InPlace,
    Managed,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunWorkspace {
    pub placement: WorkspacePlacement,
    /// Exact local working folder; absent on contained placements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_directory: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seeds: Vec<crate::FileSelection>,
    /// Local only: freeze the selected working folder's contents at the managed root.
    #[serde(default, skip_serializing_if = "is_false")]
    pub copy_root: bool,
}

const fn is_false(value: &bool) -> bool {
    !*value
}

impl RunWorkspace {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.placement == WorkspacePlacement::InPlace
            && (!self.seeds.is_empty() || self.copy_root)
        {
            return Err("in-place workspaces cannot copy seeds; select managed placement");
        }
        if self.copy_root && !self.seeds.is_empty() {
            return Err("copyRoot cannot be combined with workspace seeds");
        }
        let mut names = std::collections::BTreeSet::new();
        for selection in &self.seeds {
            let name = seed_name(selection)?;
            if !names.insert(name.to_ascii_lowercase()) {
                return Err("workspace seeds have colliding basenames");
            }
        }
        Ok(())
    }

    pub fn is_in_place(&self) -> bool {
        self.placement == WorkspacePlacement::InPlace
    }
}

pub fn seed_name(selection: &crate::FileSelection) -> Result<&str, &'static str> {
    let path = if selection.member.as_str().is_empty() {
        &selection.address.path
    } else {
        &selection.member
    };
    path.as_str()
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .ok_or("workspace seed needs a basename")
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunWorkspacesDiscovery {
    pub kind: String,
}
