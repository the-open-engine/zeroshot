use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Ignore, Result, files, invalid};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparing {
    schema_version: u32,
    state_root: PathBuf,
    filters: Ignore,
}

pub(super) async fn ensure(state: PathBuf, filters: &Ignore, directory: &Path) -> Result<()> {
    if files::optional::<bool>(&directory.join("prepared.json"))? == Some(true) {
        return Ok(());
    }
    let saved = files::optional::<Preparing>(&directory.join("preparing.json"))?;
    let (state, filters) = match saved {
        Some(saved) => {
            if saved.schema_version != 2 {
                return Err(invalid("unsupported preparation checkpoint"));
            }
            saved.filters.validate_resume(filters)?;
            (saved.state_root, saved.filters)
        }
        None => (state, filters.clone()),
    };
    super::prepare(state, directory, filters).await?;
    Ok(())
}

pub(super) fn checkpoint(
    state: &Path,
    filters: &Ignore,
    directory: &Path,
) -> Result<Option<SessionLock>> {
    ensure_directory(directory)?;
    let lock = SessionLock::acquire(directory)?;
    let saved = files::optional::<Preparing>(&directory.join("preparing.json"))?;
    verify_preparation(saved, state, filters, directory)?;
    if files::optional::<bool>(&directory.join("prepared.json"))? == Some(true) {
        return Ok(None);
    }
    files::write(
        &directory.join("preparing.json"),
        &Preparing {
            schema_version: 2,
            state_root: state.to_owned(),
            filters: filters.clone(),
        },
    )?;
    Ok(Some(lock))
}

fn ensure_directory(directory: &Path) -> Result<()> {
    if directory.try_exists()? {
        files::private_directory(directory)
    } else {
        files::create_directory(directory)
    }
}

fn verify_preparation(
    saved: Option<Preparing>,
    state: &Path,
    filters: &Ignore,
    directory: &Path,
) -> Result<()> {
    if let Some(saved) = saved {
        if saved.schema_version != 2 || saved.state_root != state || &saved.filters != filters {
            return Err(invalid(
                "initialization directory belongs to another preparation",
            ));
        }
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)? {
        if entry?.file_name() != ".init.lock" {
            return Err(invalid(
                "initialization directory is not empty or a saved preparation",
            ));
        }
    }
    Ok(())
}

pub(super) struct SessionLock(File);
impl SessionLock {
    pub(super) fn acquire(directory: &Path) -> Result<Self> {
        let file = crate::execution::platform::private_file(
            &directory.join(".init.lock"),
            crate::execution::platform::FileAccess::ReadWrite,
        )?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
            invalid("initialization session is busy; retry when the other invocation finishes")
        })?;
        Ok(Self(file))
    }
}
impl Drop for SessionLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}
