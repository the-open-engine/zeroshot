use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use openengine_cluster_protocol::{
    RunProfile, RunProfileDefaultRequest, RunProfileDefaultResult, RunProfileDeleteResult,
    RunProfileListRequest, RunProfileMutationResult, RunProfileName, RunProfileScope,
    RunProfileSelector, RunProfileSetRequest, RunProfileSummary,
};
use serde::{Deserialize, Serialize};
use serde_json::value::{RawValue, to_raw_value};

use super::support::{CommitPaths, cleanup_temporary, write_and_commit};
use super::{
    CliProfileListResult, CliProfileProblem, NativeV2CliError, absolute_user_path,
    nonempty_environment,
};

const PROFILES_FILE: &str = "profiles.json";
const PROFILES_LOCK_FILE: &str = "profiles.lock";

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredProfiles {
    #[serde(default)]
    profiles: BTreeMap<RunProfileName, Box<RawValue>>,
    #[serde(default)]
    default: Option<RunProfileName>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredProfile<G, R> {
    id: String,
    graph: G,
    runtime: R,
}

#[derive(Clone)]
pub(crate) struct LocalRunProfileStore {
    root: PathBuf,
}

#[cfg(feature = "ui")]
pub(crate) enum ProfileSaveConflict {
    Revision,
    Workspace,
}

impl LocalRunProfileStore {
    pub(crate) fn production() -> Result<Self, NativeV2CliError> {
        Ok(Self {
            root: default_config_root()?,
        })
    }

    #[cfg(any(test, feature = "ui"))]
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[cfg(feature = "ui")]
    pub(crate) fn workspace_id(&self) -> Result<String, NativeV2CliError> {
        let lock = self.lock()?;
        lock.lock_exclusive().map_err(local_io)?;
        let destination = self.root.join("ui-workspace-id");
        if let Some(id) = read_workspace_id(&destination)? {
            return Ok(id);
        }
        let id = uuid::Uuid::now_v7().to_string();
        let temporary = temporary_path(&self.root);
        let result = (|| {
            let file = owner_only_options()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(local_io)?;
            write_and_commit(
                file,
                format!("{id}\n").as_bytes(),
                CommitPaths {
                    temporary: &temporary,
                    destination: &destination,
                    parent: &self.root,
                },
            )
        })();
        cleanup_temporary(result, &temporary)?;
        Ok(id)
    }

    pub(crate) fn list(
        &self,
        request: RunProfileListRequest,
    ) -> Result<CliProfileListResult, NativeV2CliError> {
        require_local_scope(request.scope)?;
        let lock = self.lock()?;
        FileExt::lock_shared(&lock).map_err(local_io)?;
        let stored = self.read()?;
        let mut result = CliProfileListResult::default();
        for (name, value) in stored.profiles {
            let is_default = stored.default.as_ref() == Some(&name);
            match stored_profile(&value, &name) {
                Ok(profile) => result.profiles.push(RunProfileSummary {
                    id: profile.id,
                    name,
                    scope: RunProfileScope::User,
                    is_default,
                }),
                Err(_) => result.problems.push(CliProfileProblem {
                    message: format!("profile {name} has malformed metadata"),
                    name,
                    scope: RunProfileScope::User,
                    is_default,
                }),
            }
        }
        Ok(result)
    }

    pub(crate) fn show(
        &self,
        selector: RunProfileSelector,
    ) -> Result<RunProfile, NativeV2CliError> {
        require_local_scope(selector.scope)?;
        let lock = self.lock()?;
        FileExt::lock_shared(&lock).map_err(local_io)?;
        let stored = self.read()?;
        profile(&stored, &selector.name)?.ok_or_else(|| {
            NativeV2CliError::Local(format!("profile {} was not found", selector.name))
        })
    }

    pub(crate) fn set(
        &self,
        request: RunProfileSetRequest,
    ) -> Result<RunProfileMutationResult, NativeV2CliError> {
        self.set_inner(request, None)?
            .ok_or_else(|| local_message("profile conflict"))
    }

    /// Atomically protects browser edits against changes by another tab or the CLI.
    #[cfg(feature = "ui")]
    pub(crate) fn set_checked(
        &self,
        request: RunProfileSetRequest,
        expected: Option<&str>,
        workspace: &str,
    ) -> Result<Result<RunProfileMutationResult, ProfileSaveConflict>, NativeV2CliError> {
        require_local_scope(request.scope)?;
        let lock = self.lock()?;
        lock.lock_exclusive().map_err(local_io)?;
        if read_workspace_id(&self.root.join("ui-workspace-id"))?.as_deref() != Some(workspace) {
            return Ok(Err(ProfileSaveConflict::Workspace));
        }
        self.set_locked(request, Some(expected))
            .map(|saved| saved.ok_or(ProfileSaveConflict::Revision))
    }

    fn set_inner(
        &self,
        request: RunProfileSetRequest,
        expected: Option<Option<&str>>,
    ) -> Result<Option<RunProfileMutationResult>, NativeV2CliError> {
        require_local_scope(request.scope)?;
        let lock = self.lock()?;
        lock.lock_exclusive().map_err(local_io)?;
        self.set_locked(request, expected)
    }

    fn set_locked(
        &self,
        request: RunProfileSetRequest,
        expected: Option<Option<&str>>,
    ) -> Result<Option<RunProfileMutationResult>, NativeV2CliError> {
        let mut stored = self.read()?;
        let id = if let Some(expected) = expected {
            let previous = profile(&stored, &request.name)?;
            let current = previous.as_ref().map(profile_revision).transpose()?;
            if current.as_deref() != expected {
                return Ok(None);
            }
            previous.map(|profile| profile.id)
        } else {
            stored
                .profiles
                .get(&request.name)
                .map(|value| stored_profile(value, &request.name).map(|profile| profile.id))
                .transpose()?
        }
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        if request.set_default {
            stored.default = Some(request.name.clone());
        }
        let is_default = stored.default.as_ref() == Some(&request.name);
        let (value, profile) = encode_profile(id, request, is_default)?;
        stored.profiles.insert(profile.name.clone(), value);
        self.write(&stored)?;
        Ok(Some(RunProfileMutationResult { profile }))
    }

    pub(crate) fn delete(
        &self,
        selector: RunProfileSelector,
    ) -> Result<RunProfileDeleteResult, NativeV2CliError> {
        require_local_scope(selector.scope)?;
        let lock = self.lock()?;
        lock.lock_exclusive().map_err(local_io)?;
        let mut stored = self.read()?;
        let deleted = stored.profiles.remove(&selector.name).is_some();
        if stored.default.as_ref() == Some(&selector.name) {
            stored.default = None;
        }
        if deleted {
            self.write(&stored)?;
        }
        Ok(RunProfileDeleteResult { deleted })
    }

    pub(crate) fn set_default(
        &self,
        request: RunProfileDefaultRequest,
    ) -> Result<RunProfileDefaultResult, NativeV2CliError> {
        require_local_scope(request.scope)?;
        let lock = self.lock()?;
        lock.lock_exclusive().map_err(local_io)?;
        let mut stored = self.read()?;
        if let Some(name) = &request.name {
            if !stored.profiles.contains_key(name) {
                return Err(NativeV2CliError::Local(format!(
                    "profile {name} was not found"
                )));
            }
        }
        stored.default = request.name.clone();
        self.write(&stored)?;
        Ok(RunProfileDefaultResult {
            scope: RunProfileScope::User,
            name: request.name,
        })
    }

    fn lock(&self) -> Result<File, NativeV2CliError> {
        std::fs::create_dir_all(&self.root).map_err(local_io)?;
        owner_only_options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join(PROFILES_LOCK_FILE))
            .map_err(local_io)
    }

    fn read(&self) -> Result<StoredProfiles, NativeV2CliError> {
        let path = self.root.join(PROFILES_FILE);
        match File::open(path) {
            Ok(file) => serde_json::from_reader(BufReader::new(file))
                .map_err(|_| local_message("local profile store is malformed")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(StoredProfiles::default())
            }
            Err(error) => Err(local_io(error)),
        }
    }

    fn write(&self, stored: &StoredProfiles) -> Result<(), NativeV2CliError> {
        let temporary = temporary_path(&self.root);
        let mut encoded = serde_json::to_vec(stored)
            .map_err(|_| local_message("local profile store could not be encoded"))?;
        encoded.push(b'\n');
        // Accepted writes must remain readable through the store's JSON parser.
        serde_json::from_slice::<StoredProfiles>(&encoded).map_err(|_| {
            local_message("local profile store could not be read back before saving")
        })?;
        let result = (|| {
            let file = owner_only_options()
                .write(true)
                .create_new(true)
                .truncate(false)
                .open(&temporary)
                .map_err(local_io)?;
            let destination = self.root.join(PROFILES_FILE);
            write_and_commit(
                file,
                &encoded,
                CommitPaths {
                    temporary: &temporary,
                    destination: &destination,
                    parent: &self.root,
                },
            )
        })();
        cleanup_temporary(result, &temporary)
    }
}

#[cfg(feature = "ui")]
fn read_workspace_id(path: &Path) -> Result<Option<String>, NativeV2CliError> {
    use std::io::Read as _;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(local_io(error)),
    };
    if !metadata.is_file() || metadata.len() > 64 {
        return Err(local_message(
            "UI workspace identity is not a regular bounded file",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut encoded = String::new();
    options
        .open(path)
        .map_err(local_io)?
        .take(65)
        .read_to_string(&mut encoded)
        .map_err(local_io)?;
    let id = uuid::Uuid::parse_str(encoded.trim())
        .map_err(|_| local_message("UI workspace identity is malformed"))?;
    Ok(Some(id.to_string()))
}

fn encode_profile(
    id: String,
    request: RunProfileSetRequest,
    is_default: bool,
) -> Result<(Box<RawValue>, RunProfile), NativeV2CliError> {
    let value = to_raw_value(&StoredProfile {
        id,
        graph: request.graph,
        runtime: request.runtime,
    })
    .map_err(|_| local_message("local profile store could not be encoded"))?;
    let profile = decode_profile(&value, &request.name, is_default)
        .map_err(|_| local_message("local profile store could not be read back before saving"))?;
    Ok((value, profile))
}

// Borrow payload slices when inspecting metadata; only selected payloads become typed graphs.
fn stored_profile<'a>(
    value: &'a RawValue,
    name: &RunProfileName,
) -> Result<StoredProfile<&'a RawValue, &'a RawValue>, NativeV2CliError> {
    serde_json::from_str(value.get())
        .map_err(|_| local_message(format!("profile {name} has malformed metadata")))
}

fn profile(
    stored: &StoredProfiles,
    name: &RunProfileName,
) -> Result<Option<RunProfile>, NativeV2CliError> {
    let Some(value) = stored.profiles.get(name) else {
        return Ok(None);
    };
    decode_profile(value, name, stored.default.as_ref() == Some(name)).map(Some)
}

fn decode_profile(
    value: &RawValue,
    name: &RunProfileName,
    is_default: bool,
) -> Result<RunProfile, NativeV2CliError> {
    let value = stored_profile(value, name)?;
    Ok(RunProfile {
        id: value.id,
        name: name.clone(),
        scope: RunProfileScope::User,
        graph: serde_json::from_str(value.graph.get())
            .map_err(|_| local_message(format!("profile {name} has a malformed graph")))?,
        runtime: serde_json::from_str(value.runtime.get())
            .map_err(|_| local_message(format!("profile {name} has a malformed runtime")))?,
        is_default,
    })
}

fn require_local_scope(scope: RunProfileScope) -> Result<(), NativeV2CliError> {
    if scope == RunProfileScope::User {
        Ok(())
    } else {
        Err(local_message(
            "organization-scoped profiles require a hosted target",
        ))
    }
}

fn default_config_root() -> Result<PathBuf, NativeV2CliError> {
    if let Some(path) = nonempty_environment("ZEROSHOT_CONFIG_DIR") {
        return absolute_user_path(path, "profile configuration path must be absolute");
    }
    #[cfg(windows)]
    {
        let root = nonempty_environment("LOCALAPPDATA")
            .ok_or_else(|| local_message("LOCALAPPDATA is unavailable"))?;
        absolute_user_path(
            PathBuf::from(root).join("zeroshot"),
            "profile configuration path must be absolute",
        )
    }
    #[cfg(unix)]
    default_unix_config_root()
}

#[cfg(unix)]
fn default_unix_config_root() -> Result<PathBuf, NativeV2CliError> {
    if let Some(path) = nonempty_environment("XDG_CONFIG_HOME") {
        return absolute_user_path(
            PathBuf::from(path).join("zeroshot"),
            "profile configuration path must be absolute",
        );
    }
    let home = nonempty_environment("HOME")
        .ok_or_else(|| local_message("HOME and XDG_CONFIG_HOME are unavailable"))?;
    absolute_user_path(
        PathBuf::from(home).join(".config").join("zeroshot"),
        "profile configuration path must be absolute",
    )
}

fn temporary_path(root: &Path) -> PathBuf {
    root.join(format!("profiles.{}.tmp", uuid::Uuid::now_v7()))
}

/// Store files have fixed names, and a direct target's storage root is traversable by its run
/// identities, so new files are created owner-only.
#[cfg(unix)]
fn owner_only_options() -> OpenOptions {
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut options = OpenOptions::new();
    options.mode(0o600);
    options
}

#[cfg(not(unix))]
fn owner_only_options() -> OpenOptions {
    OpenOptions::new()
}

/// Makes store files left by earlier versions owner-only before a target exposes their root.
#[cfg(unix)]
pub(crate) fn secure_existing_profile_store(root: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    for name in [PROFILES_FILE, PROFILES_LOCK_FILE] {
        let file = match crate::execution::platform::open_identity(&root.join(name)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(std::io::Error::other(
                "profile store file is not a regular file owned by this account",
            ));
        }
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn local_io(error: std::io::Error) -> NativeV2CliError {
    NativeV2CliError::Local(error.to_string())
}

fn local_message(message: impl Into<String>) -> NativeV2CliError {
    NativeV2CliError::Local(message.into())
}

/// Content identity includes metadata so default changes cannot be overwritten silently.
pub(crate) fn profile_revision(value: &RunProfile) -> Result<String, NativeV2CliError> {
    use sha2::{Digest, Sha256};
    let encoded =
        serde_json::to_vec(value).map_err(|_| local_message("profile could not be encoded"))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

#[cfg(test)]
#[path = "profiles/tests.rs"]
mod tests;
