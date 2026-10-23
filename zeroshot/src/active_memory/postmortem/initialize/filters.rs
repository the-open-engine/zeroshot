use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Result, invalid};

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ignore {
    repositories: BTreeSet<String>,
    graphs: BTreeSet<String>,
}

impl Ignore {
    pub(crate) fn new(repositories: &[String], graphs: &[String]) -> Result<Self> {
        Ok(Self {
            repositories: entries(repositories)?,
            graphs: entries(graphs)?,
        })
    }
    pub(super) fn validate_resume(&self, requested: &Self) -> Result<()> {
        if requested != &Self::default() && self != requested {
            return Err(invalid(
                "ignore lists differ from the saved session; resume without flags or use a new directory",
            ));
        }
        Ok(())
    }
    pub(super) fn excludes_graph(&self, name: &str, hash: &str) -> bool {
        self.graphs.contains(name) || self.graphs.contains(hash)
    }
    pub(super) fn excludes_repository(&self, aliases: &BTreeSet<String>) -> bool {
        !self.repositories.is_disjoint(aliases)
    }
}

fn entries(values: &[String]) -> Result<BTreeSet<String>> {
    values
        .iter()
        .map(|value| {
            let value = value.trim();
            if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
                return Err(invalid(
                    "ignore entries must be nonempty bounded names without control characters",
                ));
            }
            Ok(value.to_owned())
        })
        .collect()
}

pub(super) fn repository_aliases(
    workspace: &Path,
    source: Option<&openengine_cluster_protocol::ResolvedSource>,
) -> BTreeSet<String> {
    let mut aliases = BTreeSet::from([workspace.to_string_lossy().into_owned()]);
    if let Some(name) = workspace.file_name().and_then(|name| name.to_str()) {
        aliases.insert(name.to_owned());
    }
    if let Some(source) = source {
        aliases.insert(source.repository.as_str().to_owned());
        aliases.insert(
            crate::active_memory::runtime::repository_key_for_repository(&source.repository),
        );
        if let Some(remote) = &source.remote {
            add_remote(&mut aliases, &remote.url);
        }
    }
    aliases
}

pub(super) fn add_remote(
    aliases: &mut BTreeSet<String>,
    remote: &openengine_cluster_protocol::GitRemoteUrl,
) {
    if let Ok(Some(forge)) = crate::source_code_provider::adapters::core::forge_identity(remote) {
        aliases.insert(forge.repository.as_str().to_owned());
        aliases.insert(
            crate::active_memory::runtime::repository_key_for_repository(&forge.repository),
        );
    }
    aliases.insert(crate::active_memory::runtime::repository_key_for_remote(
        remote,
    ));
}
