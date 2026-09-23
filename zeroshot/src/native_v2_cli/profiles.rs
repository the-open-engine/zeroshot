use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::BufReader;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use openengine_cluster_protocol::{
    RunProfile, RunProfileDefaultRequest, RunProfileDefaultResult, RunProfileDeleteResult,
    RunProfileListRequest, RunProfileListResult, RunProfileMutationResult, RunProfileName,
    RunProfileScope, RunProfileSelector, RunProfileSetRequest, RunProfileSummary,
};
use serde::{Deserialize, Serialize};

use super::support::{CommitPaths, cleanup_temporary, write_and_commit};
use super::{NativeV2CliError, absolute_user_path, nonempty_environment};

const PROFILES_FILE: &str = "profiles.json";
const PROFILES_LOCK_FILE: &str = "profiles.lock";

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredProfiles {
    #[serde(default)]
    profiles: BTreeMap<RunProfileName, StoredProfile>,
    #[serde(default)]
    default: Option<RunProfileName>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredProfile {
    id: String,
    graph: openengine_cluster_protocol::GraphSpec,
    runtime: openengine_cluster_protocol::RuntimePlan,
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
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let file = options.open(&temporary).map_err(local_io)?;
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
    ) -> Result<RunProfileListResult, NativeV2CliError> {
        require_local_scope(request.scope)?;
        let lock = self.lock()?;
        FileExt::lock_shared(&lock).map_err(local_io)?;
        let stored = self.read()?;
        Ok(RunProfileListResult {
            profiles: stored
                .profiles
                .iter()
                .map(|(name, profile)| RunProfileSummary {
                    id: profile.id.clone(),
                    is_default: stored.default.as_ref() == Some(name),
                    name: name.clone(),
                    scope: RunProfileScope::User,
                })
                .collect(),
        })
    }

    pub(crate) fn show(
        &self,
        selector: RunProfileSelector,
    ) -> Result<RunProfile, NativeV2CliError> {
        require_local_scope(selector.scope)?;
        let lock = self.lock()?;
        FileExt::lock_shared(&lock).map_err(local_io)?;
        let stored = self.read()?;
        profile(&stored, &selector.name).ok_or_else(|| {
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
        if let Some(expected) = expected {
            let current = profile(&stored, &request.name)
                .as_ref()
                .map(profile_revision)
                .transpose()?;
            if current.as_deref() != expected {
                return Ok(None);
            }
        }
        let id = stored
            .profiles
            .get(&request.name)
            .map(|profile| profile.id.clone())
            .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
        stored.profiles.insert(
            request.name.clone(),
            StoredProfile {
                id,
                graph: request.graph,
                runtime: request.runtime,
            },
        );
        if request.set_default {
            stored.default = Some(request.name.clone());
        }
        self.write(&stored)?;
        Ok(Some(RunProfileMutationResult {
            profile: profile(&stored, &request.name)
                .ok_or_else(|| local_message("stored profile disappeared"))?,
        }))
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
        OpenOptions::new()
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
        let result = (|| {
            let file = OpenOptions::new()
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

fn profile(stored: &StoredProfiles, name: &RunProfileName) -> Option<RunProfile> {
    let value = stored.profiles.get(name)?;
    Some(RunProfile {
        id: value.id.clone(),
        name: name.clone(),
        scope: RunProfileScope::User,
        graph: value.graph.clone(),
        runtime: value.runtime.clone(),
        is_default: stored.default.as_ref() == Some(name),
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
mod tests {
    use openengine_cluster_testkit::admission::graph_fixture;
    use openengine_cluster_testkit::assertions::AssertValue;
    use serde_json::json;

    use super::*;

    fn profile_request(name: &str, node: &str, set_default: bool) -> RunProfileSetRequest {
        RunProfileSetRequest {
            name: RunProfileName::new(name).assert_value(),
            scope: RunProfileScope::User,
            graph: graph_fixture(node, json!({"kind": "null"})),
            runtime: serde_json::from_value(json!({
                "harness": "codex",
                "provider": "openai",
                "size": "small",
                "nodes": {(node): {
                    "kind": "agent",
                    "model": "gpt-5.6-sol",
                }}
            }))
            .assert_value(),
            set_default,
        }
    }

    fn selector(name: &str) -> RunProfileSelector {
        RunProfileSelector {
            scope: RunProfileScope::User,
            name: RunProfileName::new(name).assert_value(),
        }
    }

    fn assert_malformed<T>(result: Result<T, NativeV2CliError>) {
        assert!(matches!(
            result,
            Err(NativeV2CliError::Local(message))
                if message == "local profile store is malformed"
        ));
    }

    fn exercise_default_and_delete_lifecycle(store: &LocalRunProfileStore, edited: &RunProfile) {
        store
            .set_default(RunProfileDefaultRequest {
                scope: RunProfileScope::User,
                name: Some(edited.name.clone()),
            })
            .assert_value();
        let default_alpha = store.show(selector("alpha")).assert_value();
        assert!(default_alpha.is_default);
        assert!(!store.show(selector("beta")).assert_value().is_default);
        assert_ne!(
            profile_revision(&default_alpha).assert_value(),
            profile_revision(edited).assert_value()
        );

        let cleared = store
            .set_default(RunProfileDefaultRequest {
                scope: RunProfileScope::User,
                name: None,
            })
            .assert_value();
        assert_eq!(cleared.name, None);
        assert!(!store.show(selector("alpha")).assert_value().is_default);
        store
            .set_default(RunProfileDefaultRequest {
                scope: RunProfileScope::User,
                name: Some(edited.name.clone()),
            })
            .assert_value();

        let missing_default = RunProfileName::new("missing").assert_value();
        assert!(matches!(
            store.set_default(RunProfileDefaultRequest {
                scope: RunProfileScope::User,
                name: Some(missing_default),
            }),
            Err(NativeV2CliError::Local(message)) if message == "profile missing was not found"
        ));
        assert!(store.show(selector("alpha")).assert_value().is_default);

        assert!(store.delete(selector("beta")).assert_value().deleted);
        assert!(!store.delete(selector("beta")).assert_value().deleted);
        assert!(store.delete(selector("alpha")).assert_value().deleted);
        assert!(matches!(
            store.show(selector("alpha")),
            Err(NativeV2CliError::Local(message)) if message == "profile alpha was not found"
        ));
        assert!(
            store
                .list(RunProfileListRequest {
                    scope: RunProfileScope::User,
                })
                .assert_value()
                .profiles
                .is_empty()
        );
    }

    #[cfg(feature = "ui")]
    #[test]
    fn workspace_identity_survives_restart_and_move_but_not_store_replacement() {
        let root =
            std::env::temp_dir().join(format!("zeroshot-workspace-{}", uuid::Uuid::now_v7()));
        let moved = root.with_extension("moved");
        let first = LocalRunProfileStore::new(root.clone())
            .workspace_id()
            .assert_value();
        assert_eq!(
            first,
            LocalRunProfileStore::new(root.clone())
                .workspace_id()
                .assert_value()
        );
        assert!(!root.join(PROFILES_FILE).exists());
        std::fs::rename(&root, &moved).assert_value();
        assert_eq!(
            first,
            LocalRunProfileStore::new(moved.clone())
                .workspace_id()
                .assert_value()
        );
        let replacement = LocalRunProfileStore::new(root.clone())
            .workspace_id()
            .assert_value();
        assert_ne!(first, replacement);
        std::fs::remove_dir_all(root).assert_value();
        std::fs::remove_dir_all(moved).assert_value();
    }

    #[cfg(feature = "ui")]
    #[test]
    fn concurrent_ui_hosts_share_one_durable_workspace_identity() {
        let root =
            std::env::temp_dir().join(format!("zeroshot-workspace-{}", uuid::Uuid::now_v7()));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let root = root.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    LocalRunProfileStore::new(root)
                        .workspace_id()
                        .assert_value()
                })
            })
            .collect();
        let ids: std::collections::BTreeSet<_> = threads
            .into_iter()
            .map(|thread| thread.join().assert_value())
            .collect();
        assert_eq!(ids.len(), 1);
        std::fs::remove_dir_all(root).assert_value();
    }

    #[cfg(feature = "ui")]
    #[test]
    fn malformed_workspace_identity_is_not_silently_replaced() {
        let root =
            std::env::temp_dir().join(format!("zeroshot-workspace-{}", uuid::Uuid::now_v7()));
        let store = LocalRunProfileStore::new(root.clone());
        store.workspace_id().assert_value();
        let path = root.join("ui-workspace-id");
        for malformed in ["not-an-identity".to_owned(), "x".repeat(128)] {
            std::fs::write(&path, &malformed).assert_value();
            assert!(store.workspace_id().is_err());
            assert_eq!(std::fs::read_to_string(&path).assert_value(), malformed);
        }
        std::fs::remove_dir_all(root).assert_value();
    }

    #[cfg(all(feature = "ui", unix))]
    #[test]
    fn workspace_identity_does_not_follow_a_symlink() {
        let root =
            std::env::temp_dir().join(format!("zeroshot-workspace-{}", uuid::Uuid::now_v7()));
        let store = LocalRunProfileStore::new(root.clone());
        store.workspace_id().assert_value();
        let path = root.join("ui-workspace-id");
        let outside = root.with_extension("identity");
        let id = uuid::Uuid::now_v7().to_string();
        std::fs::write(&outside, &id).assert_value();
        std::fs::remove_file(&path).assert_value();
        std::os::unix::fs::symlink(&outside, &path).assert_value();
        assert!(store.workspace_id().is_err());
        assert_eq!(std::fs::read_to_string(&outside).assert_value(), id);
        std::fs::remove_file(outside).assert_value();
        std::fs::remove_dir_all(root).assert_value();
    }

    #[test]
    fn profile_crud_defaults_and_revisions_are_durable() {
        let root = tempfile::tempdir().assert_value();
        let store = LocalRunProfileStore::new(root.path().to_path_buf());
        assert!(
            store
                .list(RunProfileListRequest {
                    scope: RunProfileScope::User,
                })
                .assert_value()
                .profiles
                .is_empty()
        );
        assert!(matches!(
            store.list(RunProfileListRequest {
                scope: RunProfileScope::Org,
            }),
            Err(NativeV2CliError::Local(message))
                if message == "organization-scoped profiles require a hosted target"
        ));

        let alpha = store
            .set(profile_request("alpha", "worker", false))
            .assert_value()
            .profile;
        let alpha_revision = profile_revision(&alpha).assert_value();
        let beta = store
            .set(profile_request("beta", "reviewer", true))
            .assert_value()
            .profile;
        assert!(beta.is_default);

        let reopened = LocalRunProfileStore::new(root.path().to_path_buf());
        let listed = reopened
            .list(RunProfileListRequest {
                scope: RunProfileScope::User,
            })
            .assert_value()
            .profiles;
        assert_eq!(
            listed
                .iter()
                .map(|profile| (profile.name.as_str(), profile.is_default))
                .collect::<Vec<_>>(),
            [("alpha", false), ("beta", true)]
        );

        let edited = reopened
            .set(profile_request("alpha", "implementer", false))
            .assert_value()
            .profile;
        assert_eq!(edited.id, alpha.id);
        assert_ne!(profile_revision(&edited).assert_value(), alpha_revision);
        exercise_default_and_delete_lifecycle(&reopened, &edited);
    }

    #[test]
    fn malformed_profile_store_fails_closed_for_reads_and_mutations() {
        let root = tempfile::tempdir().assert_value();
        let path = root.path().join(PROFILES_FILE);
        let malformed = br#"{"profiles":{},"unexpected":true}"#;
        std::fs::write(&path, malformed).assert_value();
        let store = LocalRunProfileStore::new(root.path().to_path_buf());

        assert_malformed(store.list(RunProfileListRequest {
            scope: RunProfileScope::User,
        }));
        assert_malformed(store.show(selector("alpha")));
        assert_malformed(store.set(profile_request("alpha", "worker", false)));
        assert_malformed(store.delete(selector("alpha")));
        assert_malformed(store.set_default(RunProfileDefaultRequest {
            scope: RunProfileScope::User,
            name: None,
        }));
        assert_eq!(std::fs::read(path).assert_value(), malformed);
    }

    #[cfg(feature = "ui")]
    #[test]
    fn checked_profile_edits_enforce_workspace_and_revision_preconditions() {
        let root = tempfile::tempdir().assert_value();
        let store = LocalRunProfileStore::new(root.path().to_path_buf());
        let workspace = store.workspace_id().assert_value();
        let request = profile_request("guarded", "worker", false);
        let created = match store
            .set_checked(request.clone(), None, &workspace)
            .assert_value()
        {
            Ok(created) => created.profile,
            Err(_) => panic!("new profile unexpectedly conflicted"),
        };
        let original_revision = profile_revision(&created).assert_value();

        assert!(matches!(
            store
                .set_checked(request.clone(), None, &workspace)
                .assert_value(),
            Err(ProfileSaveConflict::Revision)
        ));
        let updated = match store
            .set_checked(
                profile_request("guarded", "reviewer", false),
                Some(&original_revision),
                &workspace,
            )
            .assert_value()
        {
            Ok(updated) => updated.profile,
            Err(_) => panic!("current revision unexpectedly conflicted"),
        };
        assert_eq!(updated.id, created.id);
        assert_ne!(profile_revision(&updated).assert_value(), original_revision);
        assert!(matches!(
            store
                .set_checked(request.clone(), Some(&original_revision), &workspace)
                .assert_value(),
            Err(ProfileSaveConflict::Revision)
        ));
        assert!(matches!(
            store
                .set_checked(request, None, "different-workspace")
                .assert_value(),
            Err(ProfileSaveConflict::Workspace)
        ));
    }
}
