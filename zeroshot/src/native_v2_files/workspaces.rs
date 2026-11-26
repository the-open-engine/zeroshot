//! Admission pins shared by local execution and ordinary capsule allocation.
use std::collections::BTreeSet;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use fs2::FileExt;
use openengine_cluster_protocol::{FileEntryKind, IdempotencyKey, RunId, RunWorkspace};
use openengine_cluster_server::identity::ConnectionIdentity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use super::{FileAccess, FilePin, FilePinRequest, TargetFileHost, Result, invalid, paths};

/// Host-selected authenticated file namespace. Never constructed from submitted paths or labels.
#[derive(Clone)]
pub struct WorkspaceFiles {
    pub host: Arc<dyn TargetFileHost>,
    pub target: String,
    pub identity: ConnectionIdentity,
    pub directory: PathBuf,
}

pub struct FreezeWorkspace<'a> {
    pub run_id: &'a RunId,
    pub submission_key: &'a IdempotencyKey,
    pub workspace: &'a RunWorkspace,
    pub local_root: Option<&'a Path>,
}

#[derive(Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    run_id: RunId,
    workspace: RunWorkspace,
    local_root: Option<PathBuf>,
}

fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

impl WorkspaceFiles {
    fn access(&self) -> FileAccess<'_> {
        FileAccess {
            target: &self.target,
            identity: &self.identity,
        }
    }

    fn directory(&self) -> Result<paths::Directory> {
        paths::private_root(&self.directory)?;
        let directory = paths::Directory::open(&self.directory)?;
        directory.validate_private()?;
        Ok(directory)
    }

    /// Persist the first run identity before pinning. A lost reply or interrupted admission can
    /// therefore resume the same pins without consulting a mutable source again.
    pub fn freeze(&self, request: FreezeWorkspace<'_>) -> Result<RunId> {
        request.workspace.validate().map_err(invalid)?;
        if request.workspace.is_in_place() {
            return Ok(request.run_id.clone());
        }
        if request.workspace.copy_root != request.local_root.is_some() {
            return Err(invalid(
                "copyRoot requires an authorized local working folder",
            ));
        }
        let directory = self.directory()?;
        let lock = directory.writable_file(std::ffi::OsStr::new("admission.lock"))?;
        lock.lock_exclusive()?;
        let manifest = reserve_manifest(&directory, request)?;
        match directory.read_file(std::ffi::OsStr::new(&released_name(&manifest.run_id))) {
            Ok(marker) => {
                // Windows cannot replace the release marker while this reader is open.
                drop(marker);
                self.release_manifest(&directory, &manifest)?;
                return Err(invalid(
                    "workspace inputs were released; submit with a new submission key",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let result = self.freeze_inputs(&directory, &manifest);
        if result.is_err() {
            self.release_manifest(&directory, &manifest)?;
        }
        result?;
        Ok(manifest.run_id)
    }

    fn freeze_inputs(&self, directory: &paths::Directory, manifest: &Manifest) -> Result<()> {
        if let Some(root) = &manifest.local_root {
            let pin = pin(&manifest.run_id, 0)?;
            if !self.host.pin_exists(&self.access(), &pin)? {
                let mut transfer = self.transfer()?;
                let executable = super::transfer::pack_with_permissions(root, &mut transfer)?;
                let mut administration = GitAdministration::default();
                administration.collect(transfer.as_file_mut(), "")?;
                validate_managed_input(transfer.as_file_mut(), "", &administration.roots())?;
                write_manifest(directory, &executable_name(&manifest.run_id), &executable)?;
                transfer.rewind()?;
                self.host.stage(&self.access(), &pin, &mut transfer)?;
            }
        }
        self.freeze_selections(&manifest.run_id, &manifest.workspace.seeds)
    }

    /// Permanently release this lineage's seed bytes, retaining admission identities for replay.
    /// Retry under the same lock after partial release; file-store tombstones prevent repinning.
    pub fn release(&self, run_id: &RunId) -> Result<()> {
        let directory = self.directory()?;
        let lock = directory.writable_file(std::ffi::OsStr::new("admission.lock"))?;
        lock.lock_exclusive()?;
        let manifest: Manifest = match directory.read_file(std::ffi::OsStr::new(&run_name(run_id)))
        {
            Ok(file) => serde_json::from_reader(file)
                .map_err(|_| invalid("invalid workspace input manifest"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if manifest.run_id != *run_id {
            return Err(invalid("workspace input identity mismatch"));
        }
        self.release_manifest(&directory, &manifest)
    }

    fn release_manifest(&self, directory: &paths::Directory, manifest: &Manifest) -> Result<()> {
        write_manifest(directory, &released_name(&manifest.run_id), &true)?;
        let count = if manifest.workspace.copy_root {
            1
        } else {
            manifest.workspace.seeds.len()
        };
        for index in 0..count {
            self.host
                .release(&self.access(), &pin(&manifest.run_id, index)?)?;
        }
        match directory.remove_file(std::ffi::OsStr::new(&executable_name(&manifest.run_id))) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn freeze_selections(
        &self,
        run_id: &RunId,
        selections: &[openengine_cluster_protocol::FileSelection],
    ) -> Result<()> {
        let mut administration = GitAdministration::default();
        for (index, selection) in selections.iter().enumerate() {
            self.host.pin(
                &self.access(),
                &FilePinRequest {
                    pin: pin(run_id, index)?,
                    source: selection.clone(),
                },
            )?;
            administration.collect(
                self.pinned_transfer(run_id, index)?.as_file_mut(),
                openengine_cluster_protocol::seed_name(selection).map_err(invalid)?,
            )?;
        }
        // Separate selections can assemble an administrative directory at the managed root.
        let roots = administration.roots();
        for (index, selection) in selections.iter().enumerate() {
            validate_managed_input(
                self.pinned_transfer(run_id, index)?.as_file_mut(),
                openengine_cluster_protocol::seed_name(selection).map_err(invalid)?,
                &roots,
            )?;
        }
        Ok(())
    }

    fn pinned_transfer(&self, run_id: &RunId, index: usize) -> Result<super::FileTransfer> {
        // Read the immutable admission pin, never a second version of the library path.
        let mut transfer = self.transfer()?;
        self.host
            .read_pin(&self.access(), &pin(run_id, index)?, &mut transfer)?;
        Ok(transfer)
    }

    fn transfer(&self) -> Result<super::FileTransfer> {
        let transfers = self.directory.join("transfers");
        super::reclaim_transfers(&transfers)?;
        super::new_transfer(&transfers)
    }

    /// Reset only an explicitly managed root, preserving its leased directory identity.
    pub fn restart(&self, run_id: &RunId, destination: &Path) -> Result<()> {
        use crate::native_v2_supervisor::checkpoints::filesystem;
        // Validate every pinned byte before changing the current managed work product.
        let parent = destination
            .parent()
            .ok_or_else(|| invalid("workspace has no parent"))?;
        let stage = filesystem::private_stage(parent, ".workspace-restart-")?;
        self.materialize(run_id, stage.path())?;
        paths::reject_links(destination)?;
        filesystem::replace_children(stage.path(), destination, false)
    }

    /// Materialize into a fresh host-owned root before startup. No reusable library is modified.
    pub fn materialize(&self, run_id: &RunId, destination: &Path) -> Result<()> {
        let directory = self.directory()?;
        let manifest: Manifest =
            serde_json::from_reader(directory.read_file(std::ffi::OsStr::new(&run_name(run_id)))?)
                .map_err(|_| invalid("invalid workspace input manifest"))?;
        if manifest.run_id != *run_id {
            return Err(invalid("workspace input identity mismatch"));
        }
        if manifest.workspace.copy_root {
            // File unpack publishes a new root. Keep capsule ownership of the existing root.
            materialize_root(self, &pin(run_id, 0)?, destination)?;
        } else {
            for (index, selection) in manifest.workspace.seeds.iter().enumerate() {
                self.host.materialize(
                    &self.access(),
                    &pin(run_id, index)?,
                    &destination
                        .join(openengine_cluster_protocol::seed_name(selection).map_err(invalid)?),
                )?;
            }
        }
        Ok(())
    }
}

// Called under the admission lock: neither identity nor pins may be published before both checks.
fn reserve_manifest(
    directory: &paths::Directory,
    request: FreezeWorkspace<'_>,
) -> Result<Manifest> {
    let name = format!(
        "submission-{}.json",
        digest(request.submission_key.as_str())
    );
    let (manifest, known_submission) = match directory.read_file(std::ffi::OsStr::new(&name)) {
        Ok(file) => {
            let manifest: Manifest = serde_json::from_reader(file)
                .map_err(|_| invalid("invalid workspace input manifest"))?;
            if manifest.workspace != *request.workspace
                || manifest.local_root.as_deref() != request.local_root
            {
                return Err(invalid(
                    "workspace submission key identifies different inputs",
                ));
            }
            (manifest, true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let manifest = Manifest {
                run_id: request.run_id.clone(),
                workspace: request.workspace.clone(),
                local_root: request.local_root.map(Path::to_owned),
            };
            (manifest, false)
        }
        Err(error) => return Err(error),
    };
    let run_name = run_name(&manifest.run_id);
    let known_run = match directory.read_file(std::ffi::OsStr::new(&run_name)) {
        Ok(file) => {
            let recorded: Manifest = serde_json::from_reader(file)
                .map_err(|_| invalid("invalid workspace input manifest"))?;
            if !known_submission || recorded != manifest {
                return Err(invalid(
                    "workspace run ID already identifies a different submission",
                ));
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    // Check both identities under the admission lock before publishing manifests or pins.
    if !known_submission {
        write_manifest(directory, &name, &manifest)?;
    }
    if !known_run {
        write_manifest(directory, &run_name, &manifest)?;
    }
    Ok(manifest)
}

fn pin(run_id: &RunId, index: usize) -> Result<FilePin> {
    Ok(FilePin {
        run_id: run_id.clone(),
        operation: IdempotencyKey::new(format!("workspace-seed-{index}"))
            .map_err(|_| invalid("invalid workspace pin identity"))?,
    })
}
fn run_name(run_id: &RunId) -> String {
    format!("run-{}.json", digest(run_id.as_str()))
}
fn released_name(run_id: &RunId) -> String {
    format!("released-{}.json", digest(run_id.as_str()))
}
fn executable_name(run_id: &RunId) -> String {
    format!("executable-{}.json", digest(run_id.as_str()))
}
fn write_manifest(
    directory: &paths::Directory,
    name: &str,
    manifest: &impl Serialize,
) -> Result<()> {
    let mut temporary = paths::TemporaryFile::new(directory)?;
    serde_json::to_writer(&mut temporary.file, manifest)?;
    temporary.file.flush()?;
    temporary.commit(std::ffi::OsStr::new(name))
}
fn materialize_root(files: &WorkspaceFiles, pin: &FilePin, destination: &Path) -> Result<()> {
    use crate::native_v2_supervisor::checkpoints::filesystem;
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("workspace has no parent"))?;
    let stage = filesystem::private_stage(parent, ".workspace-seed-")?;
    let tree = stage.path().join("tree");
    files.host.materialize(&files.access(), pin, &tree)?;
    restore_executable(files, &pin.run_id, &tree)?;
    paths::reject_links(destination)?;
    filesystem::replace_children(&tree, destination, false)
}

fn restore_executable(files: &WorkspaceFiles, run_id: &RunId, tree: &Path) -> Result<()> {
    let directory = files.directory()?;
    let executable: Vec<(openengine_cluster_protocol::FilePath, u32)> =
        match directory.read_file(std::ffi::OsStr::new(&executable_name(run_id))) {
            Ok(file) => serde_json::from_reader(file)
                .map_err(|_| invalid("invalid workspace executable metadata"))?,
            // Root pins admitted by older builds did not retain executable metadata.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
    for (path, mode) in executable {
        if mode & !0o111 != 0 || path.as_str().is_empty() {
            return Err(invalid("invalid workspace executable metadata"));
        }
        let (parents, leaf) = path
            .as_str()
            .rsplit_once('/')
            .unwrap_or(("", path.as_str()));
        let mut directory = paths::Directory::open(tree)?;
        for component in parents.split('/').filter(|part| !part.is_empty()) {
            directory = directory.child(std::ffi::OsStr::new(component))?;
        }
        let file = directory.read_file(std::ffi::OsStr::new(leaf))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600 | mode))?;
        }
        #[cfg(windows)]
        let _ = file;
    }
    Ok(())
}

// Git can discover an administrative directory under any name, including the workspace root.
// Collect only its structural markers, keeping memory and open transfers bounded per input.
#[derive(Default)]
struct GitAdministration {
    heads: BTreeSet<String>,
    objects: BTreeSet<String>,
    refs: BTreeSet<String>,
    common: BTreeSet<String>,
}

impl GitAdministration {
    fn collect(&mut self, input: &mut std::fs::File, basename: &str) -> Result<()> {
        input.rewind()?;
        let index = super::transfer::copy_validated(input, &mut std::io::sink(), &mut |_| Ok(()))?;
        for indexed in index {
            let path = managed_path(basename, indexed.entry.path.as_str());
            let (parent, name) = path.rsplit_once('/').unwrap_or(("", &path));
            match (name, indexed.entry.kind) {
                ("head", FileEntryKind::File { .. }) => {
                    self.heads.insert(parent.to_owned());
                }
                ("objects", FileEntryKind::Directory) => {
                    self.objects.insert(parent.to_owned());
                }
                ("refs", FileEntryKind::Directory) => {
                    self.refs.insert(parent.to_owned());
                }
                ("commondir", FileEntryKind::File { .. }) => {
                    self.common.insert(parent.to_owned());
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn roots(mut self) -> BTreeSet<String> {
        self.heads.retain(|parent| {
            self.common.contains(parent)
                || (self.objects.contains(parent) && self.refs.contains(parent))
        });
        self.heads
    }
}

fn managed_path(basename: &str, relative: &str) -> String {
    format!("{basename}/{relative}")
        .trim_matches('/')
        .to_ascii_lowercase()
}

// Inspect the captured bytes, including a selection's destination name. Git metadata can
// redirect ordinary relative operations outside the managed tree, even in nested repositories.
fn validate_managed_input(
    input: &mut std::fs::File,
    basename: &str,
    roots: &BTreeSet<String>,
) -> Result<()> {
    input.rewind()?;
    let index = super::transfer::copy_validated(input, &mut std::io::sink(), &mut |_| Ok(()))?;
    for indexed in index {
        let entry = indexed.entry;
        let path = managed_path(basename, entry.path.as_str());
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", &path));
        let git_file = name == ".git" && !matches!(entry.kind, FileEntryKind::Directory);
        let administration = roots.contains(parent) || path.split('/').any(|part| part == ".git");
        if git_file || (administration && matches!(name, "commondir" | "gitdir")) {
            return Err(git_redirect());
        }
        if let FileEntryKind::File { byte_length } = entry.kind
            && administration
            && matches!(name, "config" | "config.worktree")
        {
            input.seek(SeekFrom::Start(indexed.input_offset))?;
            reject_git_redirects(&mut input.take(byte_length.get()))?;
        }
    }
    Ok(())
}

fn git_redirect() -> std::io::Error {
    invalid(
        "managed inputs cannot retain external Git administration (.git file, gitdir, commondir or worktree/include configuration); use in-place placement or copy files without Git metadata",
    )
}

// Deliberately conservative: refuse these identifiers anywhere in a Git config, including
// comments/values, rather than partially parsing Git syntax or following includes. Streaming
// keeps even a very large captured config bounded, and file-only admission never needs Git.
fn reject_git_redirects(input: &mut dyn Read) -> Result<()> {
    let mut word = Vec::new();
    for byte in BufReader::new(input)
        .bytes()
        .chain(std::iter::once(Ok(b' ')))
    {
        let byte = byte?;
        if byte.is_ascii_alphanumeric() || byte == b'-' {
            if word.len() < 10 {
                word.push(byte.to_ascii_lowercase());
            }
        } else {
            if matches!(word.as_slice(), b"worktree" | b"include" | b"includeif") {
                return Err(git_redirect());
            }
            word.clear();
        }
    }
    Ok(())
}
