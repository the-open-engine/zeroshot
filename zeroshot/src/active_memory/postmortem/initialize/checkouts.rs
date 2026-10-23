use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{Result, Session, files, filters, invalid};

#[derive(Clone)]
pub(super) struct Checkout {
    pub(super) workspace: PathBuf,
    pub(super) repository: String,
    pub(super) aliases: BTreeSet<String>,
    pub(super) remote_key: Option<String>,
}

pub(super) type Cache = BTreeMap<PathBuf, std::result::Result<Checkout, String>>;

pub(super) async fn resolve(
    recorded: &Path,
    roots: (&Path, &Path),
    cache: &mut Cache,
) -> Result<Checkout> {
    if !cache.contains_key(recorded) {
        let resolved = probe(recorded, roots)
            .await
            .map_err(|error| error.to_string());
        cache.insert(recorded.to_owned(), resolved);
    }
    cache[recorded].clone().map_err(invalid)
}

async fn probe(recorded: &Path, roots: (&Path, &Path)) -> Result<Checkout> {
    let checkout = read_identity(recorded)?;
    let workspace = &checkout.workspace;
    super::validate_paths(workspace, roots.0, roots.1)?;
    files::source_digest(workspace)
        .await
        .map_err(|_| invalid("recorded checkout is unavailable or has unsupported source"))?;
    Ok(checkout)
}

fn read_identity(recorded: &Path) -> Result<Checkout> {
    if !recorded.is_absolute() {
        return Err(invalid("recorded workspace must be absolute"));
    }
    let (workspace, source) =
        crate::native_v2_local::local_resolved_source(recorded, Path::new("git"))
            .map_err(|_| invalid("recorded checkout is unavailable"))?;
    let repository =
        crate::active_memory::runtime::repository_key_for_source(source.as_ref(), &workspace);
    let mut aliases = filters::repository_aliases(&workspace, source.as_ref());
    aliases.insert(repository.clone());
    let remote = crate::native_v2_local::local_memory_remote(&workspace);
    let remote_key = remote
        .as_ref()
        .map(crate::active_memory::runtime::repository_key_for_remote);
    if let Some(remote) = remote {
        filters::add_remote(&mut aliases, &remote);
    }
    Ok(Checkout {
        workspace,
        repository,
        aliases,
        remote_key,
    })
}

pub(super) async fn sources_digest(
    session: &Session,
    workspaces: &BTreeSet<PathBuf>,
) -> Result<String> {
    let mut checked = BTreeSet::new();
    for run in session
        .runs
        .iter()
        .filter(|run| workspaces.contains(&run.workspace))
    {
        if !checked.insert(&run.workspace) {
            continue;
        }
        let current = read_identity(&run.workspace)?;
        if current.workspace != run.workspace
            || Some(&current.repository) != run.repository.as_ref()
            || current.remote_key != run.remote_key
        {
            return Err(invalid(
                "recorded checkout repository identity changed; use a new initialization directory after restoring or excluding it",
            ));
        }
    }
    files::sources_digest(workspaces).await
}
