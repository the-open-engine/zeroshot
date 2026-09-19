use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use openengine_cluster_protocol::{RunConnectionRequirements, RunId, is_canonical_uuid_v7};
use serde::{Deserialize, Serialize};

use super::contract::{normalize_origin, prepare_target, validate_target_name};
use super::TargetAdd;
use super::{TargetAccess, TargetConnectorError, TargetRecord};

const REGISTRY_VERSION: u32 = 5;
const LEGACY_REGISTRY_VERSION: u32 = 4;
const MAX_REGISTRY_BYTES: u64 = 1024 * 1024;
const RECOVERY_AUTHORIZATION_VERSION: u32 = 1;
const MAX_RECOVERY_AUTHORIZATION_BYTES: u64 = 1024 * 1024;

pub trait TargetRegistry: Send + Sync {
    fn insert(&self, target: TargetRecord) -> Result<(), TargetConnectorError>;
    fn get(&self, name: &str) -> Result<TargetRecord, TargetConnectorError>;
    fn record_recovery_authorization(
        &self,
        target_id: &str,
        run_id: &RunId,
        requirements: &RunConnectionRequirements,
    ) -> Result<(), TargetConnectorError>;
    fn recovery_authorization(
        &self,
        target_id: &str,
        run_id: &RunId,
    ) -> Result<RunConnectionRequirements, TargetConnectorError>;
    fn remove_recovery_authorization(
        &self,
        target_id: &str,
        run_id: &RunId,
    ) -> Result<(), TargetConnectorError>;
}

#[derive(Clone, Debug)]
pub struct FileTargetRegistry {
    path: PathBuf,
}

impl FileTargetRegistry {
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn with_state<T>(
        &self,
        mutate: bool,
        operation: impl FnOnce(&mut RegistryState) -> Result<T, TargetConnectorError>,
    ) -> Result<T, TargetConnectorError> {
        let parent = self
            .path
            .parent()
            .ok_or(TargetConnectorError::RegistryPath("path has no parent"))?;
        create_private_directory(parent)?;
        let lock = open_lock(&self.path.with_extension("lock"))?;
        lock_registry(&lock, true)?;
        let (mut state, initialized) = initialize_registry(&self.path)?;
        let result = operation(&mut state)?;
        if mutate || initialized {
            state.version = REGISTRY_VERSION;
            write_registry(&self.path, &state)?;
        }
        Ok(result)
    }
}

impl TargetRegistry for FileTargetRegistry {
    fn insert(&self, target: TargetRecord) -> Result<(), TargetConnectorError> {
        self.with_state(true, |state| {
            if state.targets.contains_key(&target.name) {
                return Err(TargetConnectorError::AlreadyExists(target.name));
            }
            state.targets.insert(target.name.clone(), target);
            Ok(())
        })
    }

    fn get(&self, name: &str) -> Result<TargetRecord, TargetConnectorError> {
        self.with_state(false, |state| {
            state
                .targets
                .get(name)
                .cloned()
                .ok_or_else(|| TargetConnectorError::NotFound(name.to_owned()))
        })
    }

    fn record_recovery_authorization(
        &self,
        target_id: &str,
        run_id: &RunId,
        requirements: &RunConnectionRequirements,
    ) -> Result<(), TargetConnectorError> {
        if !canonical_requirements(requirements) {
            return Err(TargetConnectorError::RecoveryAuthorizationInvalid);
        }
        let path = recovery_authorization_path(&self.path, target_id, run_id)?;
        let _lock = self.lock_recovery_authorizations()?;
        if let Some(stored) = read_recovery_authorization(&path)? {
            return if stored.matches(target_id, run_id, requirements) {
                Ok(())
            } else {
                Err(TargetConnectorError::RecoveryAuthorizationMismatch)
            };
        }
        let authorization = RecoveryAuthorization {
            version: RECOVERY_AUTHORIZATION_VERSION,
            target_id: target_id.to_owned(),
            run_id: run_id.clone(),
            connection_requirements: requirements.clone(),
        };
        write_recovery_authorization(&path, &authorization)
    }

    fn recovery_authorization(
        &self,
        target_id: &str,
        run_id: &RunId,
    ) -> Result<RunConnectionRequirements, TargetConnectorError> {
        let path = recovery_authorization_path(&self.path, target_id, run_id)?;
        let _lock = self.lock_recovery_authorizations()?;
        let authorization = read_recovery_authorization(&path)?
            .ok_or(TargetConnectorError::RecoveryAuthorizationUnavailable)?;
        if !authorization.matches_identity(target_id, run_id) {
            return Err(TargetConnectorError::RecoveryAuthorizationInvalid);
        }
        Ok(authorization.connection_requirements)
    }

    fn remove_recovery_authorization(
        &self,
        target_id: &str,
        run_id: &RunId,
    ) -> Result<(), TargetConnectorError> {
        let path = recovery_authorization_path(&self.path, target_id, run_id)?;
        let _lock = self.lock_recovery_authorizations()?;
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(TargetConnectorError::RegistryIo(error));
            }
        }
        Ok(())
    }
}

impl FileTargetRegistry {
    fn lock_recovery_authorizations(&self) -> Result<File, TargetConnectorError> {
        let parent = self
            .path
            .parent()
            .ok_or(TargetConnectorError::RegistryPath("path has no parent"))?;
        create_private_directory(parent)?;
        let lock = open_lock(&self.path.with_extension("recovery.lock"))?;
        lock_registry(&lock, true)?;
        Ok(lock)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RecoveryAuthorization {
    version: u32,
    target_id: String,
    run_id: RunId,
    connection_requirements: RunConnectionRequirements,
}

impl RecoveryAuthorization {
    fn matches_identity(&self, target_id: &str, run_id: &RunId) -> bool {
        self.version == RECOVERY_AUTHORIZATION_VERSION
            && self.target_id == target_id
            && self.run_id == *run_id
            && canonical_requirements(&self.connection_requirements)
    }

    fn matches(
        &self,
        target_id: &str,
        run_id: &RunId,
        requirements: &RunConnectionRequirements,
    ) -> bool {
        self.matches_identity(target_id, run_id) && self.connection_requirements == *requirements
    }
}

fn recovery_authorization_path(
    registry_path: &Path,
    target_id: &str,
    run_id: &RunId,
) -> Result<PathBuf, TargetConnectorError> {
    if !valid_uuid(target_id) || !is_canonical_uuid_v7(run_id) {
        return Err(TargetConnectorError::RecoveryAuthorizationInvalid);
    }
    Ok(registry_path
        .with_extension("recovery")
        .join(target_id)
        .join(format!("{}.json", run_id.as_str())))
}

fn canonical_requirements(requirements: &RunConnectionRequirements) -> bool {
    requirements.values().all(|fields| {
        !fields.is_empty()
            && fields
                .windows(2)
                .all(|pair| pair[0].as_str() < pair[1].as_str())
    })
}

fn read_recovery_authorization(
    path: &Path,
) -> Result<Option<RecoveryAuthorization>, TargetConnectorError> {
    let Some(mut file) = open_registry(path)? else {
        return Ok(None);
    };
    let metadata = file.metadata().map_err(TargetConnectorError::RegistryIo)?;
    if metadata.len() > MAX_RECOVERY_AUTHORIZATION_BYTES {
        return Err(TargetConnectorError::RecoveryAuthorizationInvalid);
    }
    let capacity = usize::try_from(metadata.len())
        .map_err(|_| TargetConnectorError::RecoveryAuthorizationInvalid)?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)
        .map_err(TargetConnectorError::RegistryIo)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| TargetConnectorError::RecoveryAuthorizationInvalid)
}

fn write_recovery_authorization(
    path: &Path,
    authorization: &RecoveryAuthorization,
) -> Result<(), TargetConnectorError> {
    let parent = path
        .parent()
        .ok_or(TargetConnectorError::RecoveryAuthorizationInvalid)?;
    create_private_directory(parent)?;
    let bytes = serde_json::to_vec(authorization)
        .map_err(|_| TargetConnectorError::RecoveryAuthorizationInvalid)?;
    if bytes.len() as u64 > MAX_RECOVERY_AUTHORIZATION_BYTES {
        return Err(TargetConnectorError::RecoveryAuthorizationInvalid);
    }
    write_private_file(path, &bytes)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RegistryState {
    version: u32,
    targets: BTreeMap<String, TargetRecord>,
}

impl Default for RegistryState {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            targets: BTreeMap::new(),
        }
    }
}

pub fn default_target_registry_path() -> Result<PathBuf, TargetConnectorError> {
    if let Some(path) = nonempty_env("ZEROSHOT_CONFIG_DIR") {
        return Ok(PathBuf::from(path).join("targets.json"));
    }
    platform_config_root().map(|root| root.join("zeroshot").join("targets.json"))
}

pub(super) fn open_lock(path: &Path) -> Result<File, TargetConnectorError> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    set_private_file_mode(&mut options);
    options.open(path).map_err(TargetConnectorError::RegistryIo)
}

pub(super) fn lock_registry(lock: &File, exclusive: bool) -> Result<(), TargetConnectorError> {
    if exclusive {
        lock.lock_exclusive()
            .map_err(TargetConnectorError::RegistryIo)
    } else {
        FileExt::lock_shared(lock).map_err(TargetConnectorError::RegistryIo)
    }
}

fn initialize_registry(path: &Path) -> Result<(RegistryState, bool), TargetConnectorError> {
    let mut state = read_registry(path)?;
    let missing_cloud = !state.targets.contains_key("cloud");
    let initialized = state.version == LEGACY_REGISTRY_VERSION || missing_cloud;
    if missing_cloud {
        let cloud = prepare_target(TargetAdd {
            name: "cloud".to_owned(),
            url: "https://api.cloud.zeroshot.sh".to_owned(),
            direct: false,
        })?;
        state.targets.insert(cloud.name.clone(), cloud);
    }
    Ok((state, initialized))
}

fn read_registry(path: &Path) -> Result<RegistryState, TargetConnectorError> {
    let Some(mut file) = open_registry(path)? else {
        return Ok(RegistryState::default());
    };
    let metadata = file.metadata().map_err(TargetConnectorError::RegistryIo)?;
    if metadata.len() > MAX_REGISTRY_BYTES {
        return Err(TargetConnectorError::RegistryTooLarge);
    }
    let capacity =
        usize::try_from(metadata.len()).map_err(|_| TargetConnectorError::RegistryTooLarge)?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)
        .map_err(TargetConnectorError::RegistryIo)?;
    let state: RegistryState =
        serde_json::from_slice(&bytes).map_err(TargetConnectorError::RegistryJson)?;
    validate_registry_state(&state)?;
    Ok(state)
}

fn open_registry(path: &Path) -> Result<Option<File>, TargetConnectorError> {
    match File::open(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(TargetConnectorError::RegistryIo(error)),
    }
}

fn validate_registry_state(state: &RegistryState) -> Result<(), TargetConnectorError> {
    if !matches!(state.version, REGISTRY_VERSION | LEGACY_REGISTRY_VERSION) {
        return Err(malformed_registry("unsupported target registry version"));
    }
    for (name, target) in &state.targets {
        if name != &target.name
            || validate_target_name(name).is_err()
            || !matches!(normalize_origin(&target.origin), Ok(origin) if origin == target.origin)
            || !valid_uuid(&target.id)
            || !valid_target_access(&target.access)
        {
            return Err(malformed_registry("invalid stored target record"));
        }
    }
    Ok(())
}

fn valid_target_access(access: &TargetAccess) -> bool {
    match access {
        TargetAccess::Hosted { device_token } => valid_uuid(device_token),
        TargetAccess::Direct => true,
    }
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()
            }
        })
}

fn malformed_registry(message: &'static str) -> TargetConnectorError {
    TargetConnectorError::RegistryJson(serde_json::Error::io(std::io::Error::other(message)))
}

fn write_registry(path: &Path, state: &RegistryState) -> Result<(), TargetConnectorError> {
    let bytes = serde_json::to_vec_pretty(state).map_err(TargetConnectorError::RegistryJson)?;
    if bytes.len() as u64 > MAX_REGISTRY_BYTES {
        return Err(TargetConnectorError::RegistryTooLarge);
    }
    write_private_file(path, &bytes)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<(), TargetConnectorError> {
    let mut suffix = [0_u8; 8];
    getrandom::fill(&mut suffix).map_err(|_| TargetConnectorError::Randomness)?;
    let temporary = path.with_extension(format!("tmp-{}", encode_hex(&suffix)));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    set_private_file_mode(&mut options);
    let result = (|| {
        let mut file = options
            .open(&temporary)
            .map_err(TargetConnectorError::RegistryIo)?;
        file.write_all(bytes)
            .map_err(TargetConnectorError::RegistryIo)?;
        file.sync_all().map_err(TargetConnectorError::RegistryIo)?;
        std::fs::rename(&temporary, path).map_err(TargetConnectorError::RegistryIo)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub(super) fn encode_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut result, "{byte:02x}");
    }
    result
}

pub(super) fn create_private_directory(path: &Path) -> Result<(), TargetConnectorError> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    set_private_directory_mode(&mut builder);
    builder
        .create(path)
        .map_err(TargetConnectorError::RegistryIo)
}

#[cfg(unix)]
fn set_private_file_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

#[cfg(not(unix))]
fn set_private_file_mode(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn set_private_directory_mode(builder: &mut std::fs::DirBuilder) {
    use std::os::unix::fs::DirBuilderExt;
    builder.mode(0o700);
}

#[cfg(not(unix))]
fn set_private_directory_mode(_builder: &mut std::fs::DirBuilder) {}

fn nonempty_env(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

#[cfg(target_os = "windows")]
fn platform_config_root() -> Result<PathBuf, TargetConnectorError> {
    nonempty_env("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or(TargetConnectorError::RegistryPath(
            "LOCALAPPDATA is unavailable",
        ))
}

#[cfg(target_os = "macos")]
fn platform_config_root() -> Result<PathBuf, TargetConnectorError> {
    nonempty_env("HOME")
        .map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        })
        .ok_or(TargetConnectorError::RegistryPath("HOME is unavailable"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_config_root() -> Result<PathBuf, TargetConnectorError> {
    if let Some(path) = nonempty_env("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path));
    }
    nonempty_env("HOME")
        .map(|home| PathBuf::from(home).join(".config"))
        .ok_or(TargetConnectorError::RegistryPath("HOME is unavailable"))
}
