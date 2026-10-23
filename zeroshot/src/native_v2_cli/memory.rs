use std::path::PathBuf;

use serde_json::Value;

use super::NativeV2CliError;
use crate::active_memory::postmortem::initialize;

/// Local bridge used by the dedicated memory initialization skill.
#[derive(Clone, Debug, Eq, PartialEq, clap::Subcommand)]
pub enum MemoryCommand {
    /// Initialize local Active Memory or resume its saved work.
    ///
    /// Creates a private session directory on the first call. Later calls return pending agent
    /// tasks, validate their responses, and publish after complete maintenance. The
    /// zeroshot-memory-init skill orchestrates the specialists using the host's connections.
    /// This command makes no model calls. It discovers all finished local runs and uses their
    /// recorded checkouts for current-source maintenance.
    Init {
        /// Repositories to exclude by inventory alias: name, owner/repo, absolute path or repo ID.
        /// Comma-separated or repeated; saved with the session. Matching is exact and case-sensitive.
        #[arg(long, value_name = "REPO", value_delimiter = ',', num_args = 1..)]
        ignore_repos: Vec<String>,
        /// Graphs to exclude by exact authored root name or graph hash shown in the inventory.
        /// Comma-separated or repeated; saved with the session. Matching is exact and case-sensitive.
        #[arg(long, value_name = "GRAPH", value_delimiter = ',', num_args = 1..)]
        ignore_graphs: Vec<String>,
        /// Private session directory outside repository checkouts; created if it does not exist.
        #[arg(long, value_name = "DIR")]
        directory: PathBuf,
    },
}

pub(super) async fn execute(command: &MemoryCommand) -> Result<Value, NativeV2CliError> {
    let MemoryCommand::Init {
        ignore_repos,
        ignore_graphs,
        directory,
    } = command;
    let filters = initialize::Ignore::new(ignore_repos, ignore_graphs)
        .map_err(|error| NativeV2CliError::Local(error.to_string()))?;
    initialize::init(
        super::support::default_local_state_root()?,
        directory,
        filters,
    )
    .await
    .map_err(|error| NativeV2CliError::Local(error.to_string()))
}
