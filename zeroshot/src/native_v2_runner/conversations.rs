//! Private provider files carried beside a checkpoint's workspace, never in its execution seed.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use openengine_cluster_protocol::NodeName;
use serde::{Deserialize, Serialize};

use crate::native_v2_contract::{NodeInstanceId, TokenUsageDelta};

mod operation;
pub(crate) use operation::ConversationIo;

pub const MAX_CONVERSATION_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_FILES: usize = 65_536;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

/// A host stores this complete directory outside its exported workspace tree and returns it on
/// restore. Files are raw provider bytes; the directory must remain alive until capture completes.
#[derive(Clone)]
pub struct ConversationFiles {
    directory: PathBuf,
    _stage: Option<Arc<tempfile::TempDir>>,
}

impl ConversationFiles {
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory: directory.canonicalize().unwrap_or(directory),
            _stage: None,
        }
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn nested(mut owner: Self, directory: PathBuf) -> Self {
        owner.directory = directory.canonicalize().unwrap_or(directory);
        owner
    }

    pub(crate) fn staged(stage: tempfile::TempDir) -> Self {
        Self {
            directory: stage
                .path()
                .canonicalize()
                .unwrap_or_else(|_| stage.path().to_owned()),
            _stage: Some(Arc::new(stage)),
        }
    }

    pub(crate) fn conversation(
        &self,
        instance: NodeInstanceId,
        node: &NodeName,
    ) -> Result<ConversationSource, &'static str> {
        let directory = self.directory.join(instance.to_string());
        let file = fs::File::open(directory.join("conversation.json"))
            .map_err(|_| "checkpoint has no conversation")?;
        let manifest: Manifest = serde_json::from_reader(file.take(MAX_MANIFEST_BYTES))
            .map_err(|_| "checkpoint conversation metadata is invalid")?;
        if manifest.node != *node || manifest.format != 1 {
            return Err("checkpoint conversation metadata is incompatible");
        }
        let files = manifest
            .files
            .into_iter()
            .map(|name| {
                if name.contains('\\') {
                    return Err("conversation contains an unsafe stored name");
                }
                let path = PathBuf::from(name);
                safe_relative(&path)
                    .then_some(path)
                    .ok_or("conversation contains an unsafe stored name")
            })
            .collect::<Result<Vec<_>, _>>()?;
        validate_files(&directory.join("files"), &files, &ConversationIo::default())?;
        Ok(ConversationSource {
            metadata: manifest.metadata,
            storage: directory.join("files"),
            files,
            _retained: Some(self.clone()),
            private_home: None,
        })
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ConversationMetadata {
    pub harness: String,
    pub id: String,
    pub usage: Option<TokenUsageDelta>,
}

/// Adapter-selected names relative to its conversation storage. Adapters never pack or edit bytes.
#[derive(Clone)]
pub struct ConversationSource {
    pub metadata: ConversationMetadata,
    pub storage: PathBuf,
    pub files: Vec<PathBuf>,
    _retained: Option<ConversationFiles>,
    private_home: Option<PathBuf>,
}

impl ConversationSource {
    #[must_use]
    pub fn new(metadata: ConversationMetadata, storage: PathBuf, files: Vec<PathBuf>) -> Self {
        Self {
            metadata,
            storage,
            files,
            _retained: None,
            private_home: None,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Manifest {
    format: u32,
    node: NodeName,
    metadata: ConversationMetadata,
    files: Vec<String>,
}

#[derive(Default)]
pub struct ConversationCapture {
    pub files: Option<ConversationFiles>,
    pub notices: Vec<(NodeName, &'static str)>,
}

pub(crate) type ConversationSources =
    BTreeMap<NodeInstanceId, (NodeName, Result<ConversationSource, &'static str>)>;

#[cfg(test)]
pub(crate) fn capture(sources: ConversationSources) -> ConversationCapture {
    capture_until_stopped(sources, &ConversationIo::default())
}

pub(crate) fn capture_until_stopped(
    sources: ConversationSources,
    operation: &ConversationIo,
) -> ConversationCapture {
    let Ok(stage) = tempfile::Builder::new()
        .prefix("zeroshot-conversations-")
        .tempdir()
    else {
        let mut result = ConversationCapture::default();
        result.notices.extend(
            sources
                .into_values()
                .map(|(node, _)| (node, "conversation staging is unavailable")),
        );
        return result;
    };
    capture_in(stage, sources, operation)
}

fn capture_in(
    stage: tempfile::TempDir,
    sources: ConversationSources,
    operation: &ConversationIo,
) -> ConversationCapture {
    let mut result = ConversationCapture::default();
    for (instance, (node, source)) in sources {
        let directory = stage.path().join(instance.to_string());
        let saved = source.and_then(|source| {
            operation.check()?;
            check_storage(&source.storage, source.private_home.as_deref())?;
            validate_files(&source.storage, &source.files, operation)?;
            copy_files(&source, &directory.join("files"), operation)?;
            operation.check()?;
            let manifest = Manifest {
                format: 1,
                node: node.clone(),
                metadata: source.metadata,
                files: source
                    .files
                    .iter()
                    .map(|name| stored_name(name))
                    .collect::<Result<_, _>>()?,
            };
            let mut file = crate::execution::platform::private_file(
                &directory.join("conversation.json"),
                crate::execution::platform::FileAccess::CreateNew,
            )
            .map_err(|_| "conversation metadata could not be saved")?;
            serde_json::to_writer(&mut file, &manifest)
                .map_err(|_| "conversation metadata could not be saved")?;
            file.flush()
                .map_err(|_| "conversation metadata could not be saved")
        });
        if let Err(reason) = saved {
            let _ = fs::remove_dir_all(directory);
            result.notices.push((node, reason));
        }
    }
    result.files = Some(ConversationFiles::staged(stage));
    result
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && (cfg!(windows) || !path.to_string_lossy().contains('\\'))
        && !path.to_string_lossy().contains(':')
}

fn stored_name(path: &Path) -> Result<String, &'static str> {
    if !safe_relative(path) {
        return Err("conversation contains an unsafe stored name");
    }
    path.components()
        .map(|part| {
            part.as_os_str()
                .to_str()
                .ok_or("conversation filename cannot be stored")
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}

fn checked_path(root: &Path, name: &Path) -> Result<PathBuf, &'static str> {
    if !safe_relative(name) {
        return Err("conversation contains an unsafe stored name");
    }
    if fs::symlink_metadata(root).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err("conversation storage is a symbolic link");
    }
    let mut path = root.to_owned();
    for part in name.components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("conversation contains a symbolic link");
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err("conversation files are unavailable"),
        }
    }
    Ok(path)
}

fn validate_files(
    root: &Path,
    files: &[PathBuf],
    operation: &ConversationIo,
) -> Result<(), &'static str> {
    if files.is_empty() {
        return Err("provider conversation files are unavailable");
    }
    if files.len() > MAX_FILES {
        return Err("conversation has too many files");
    }
    let mut size = 0_u64;
    let mut names = std::collections::BTreeSet::new();
    for name in files {
        operation.check()?;
        if !names.insert(name) {
            return Err("conversation contains duplicate stored names");
        }
        let path = checked_path(root, name)?;
        let meta = fs::symlink_metadata(path).map_err(|_| "conversation files are unavailable")?;
        if !meta.is_file() {
            return Err("conversation contains a non-file entry");
        }
        size = size
            .checked_add(meta.len())
            .ok_or("conversation exceeds the 1 GiB cap")?;
        if size > MAX_CONVERSATION_BYTES {
            return Err("conversation exceeds the 1 GiB cap");
        }
    }
    Ok(())
}

fn check_storage(storage: &Path, private_home: Option<&Path>) -> Result<(), &'static str> {
    if let Some(home) = private_home {
        let relative = storage
            .strip_prefix(home)
            .map_err(|_| "conversation storage is outside the private provider home")?;
        // Even storage equal to the home must reject a symlink at the trusted boundary.
        if relative.as_os_str().is_empty() {
            if fs::symlink_metadata(home).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Err("conversation storage is a symbolic link");
            }
        } else {
            checked_path(home, relative)?;
        }
    }
    Ok(())
}

fn copy_files(
    source: &ConversationSource,
    destination: &Path,
    operation: &ConversationIo,
) -> Result<(), &'static str> {
    let mut remaining = MAX_CONVERSATION_BYTES;
    let mut buffer = [0; 64 * 1024];
    for name in &source.files {
        operation.check()?;
        // Selection precedes this copy. Recheck the complete private-home path at the read.
        check_storage(&source.storage, source.private_home.as_deref())?;
        let path = checked_path(&source.storage, name)?;
        let target = checked_path(destination, name)?;
        crate::execution::platform::private_directory(
            target.parent().ok_or("invalid conversation name")?,
        )
        .map_err(|_| "conversation files could not be restored")?;
        let mut input = fs::File::open(path).map_err(|_| "conversation files are unavailable")?;
        let mut output = crate::execution::platform::private_file(
            &target,
            crate::execution::platform::FileAccess::CreateNew,
        )
        .map_err(|_| "conversation files could not be restored")?;
        loop {
            operation.check()?;
            let limit = buffer.len().min((remaining + 1) as usize);
            let read = input
                .read(&mut buffer[..limit])
                .map_err(|_| "conversation files are unavailable")?;
            if read == 0 {
                break;
            }
            remaining = remaining
                .checked_sub(read as u64)
                .ok_or("conversation exceeds the 1 GiB cap")?;
            output
                .write_all(&buffer[..read])
                .map_err(|_| "conversation files could not be restored")?;
        }
    }
    operation.check()
}

/// Pure provider layout: select only files belonging to the supplied conversation identifier.
#[derive(Clone)]
pub(crate) struct ConversationLayout {
    pub(crate) harness: &'static str,
    pub(crate) storage: PathBuf,
    pub(crate) select: fn(&Path, &str) -> io::Result<Vec<PathBuf>>,
}

#[derive(Clone)]
pub(crate) struct ConversationOwner {
    pub(crate) home: PathBuf,
    pub(crate) uid: u32,
    pub(crate) gid: u32,
}

struct ConversationRestore<'a> {
    source: &'a ConversationSource,
    private_home: Option<&'a Path>,
    operation: &'a ConversationIo,
}

impl ConversationLayout {
    fn validate_metadata(&self, metadata: &ConversationMetadata) -> Result<(), &'static str> {
        if self.harness == "codex" && metadata.usage.is_none() {
            return Err("Codex cumulative usage baseline is unavailable for the checkpoint turn");
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn capture(
        &self,
        metadata: ConversationMetadata,
    ) -> Result<ConversationSource, &'static str> {
        self.capture_in_home(metadata, None)
    }

    pub(crate) fn capture_in_home(
        &self,
        metadata: ConversationMetadata,
        private_home: Option<PathBuf>,
    ) -> Result<ConversationSource, &'static str> {
        check_storage(&self.storage, private_home.as_deref())?;
        self.validate_metadata(&metadata)?;
        if !valid_id(&metadata.id) {
            return Err("provider conversation identifier is invalid");
        }
        let files = (self.select)(&self.storage, &metadata.id)
            .map_err(|_| "provider conversation layout cannot be captured")?;
        let mut source = ConversationSource::new(metadata, self.storage.clone(), files);
        source.private_home = private_home;
        Ok(source)
    }

    #[cfg(test)]
    pub(crate) fn restore(
        &self,
        source: &ConversationSource,
        owner: Option<&ConversationOwner>,
    ) -> Result<(), &'static str> {
        self.restore_until_stopped(source, owner, &ConversationIo::default())
    }

    pub(crate) fn restore_until_stopped(
        &self,
        source: &ConversationSource,
        owner: Option<&ConversationOwner>,
        operation: &ConversationIo,
    ) -> Result<(), &'static str> {
        self.restore_with_ownership(
            ConversationRestore {
                source,
                private_home: owner.map(|owner| owner.home.as_path()),
                operation,
            },
            |path| {
                #[cfg(target_os = "linux")]
                if let Some(owner) = owner {
                    std::os::unix::fs::lchown(path, Some(owner.uid), Some(owner.gid))?;
                }
                #[cfg(not(target_os = "linux"))]
                let _ = (owner.map(|owner| (owner.uid, owner.gid)), path);
                Ok(())
            },
        )
    }

    fn restore_with_ownership(
        &self,
        request: ConversationRestore<'_>,
        mut assign_owner: impl FnMut(&Path) -> io::Result<()>,
    ) -> Result<(), &'static str> {
        let ConversationRestore {
            source,
            private_home,
            operation,
        } = request;
        operation.check()?;
        self.validate_metadata(&source.metadata)?;
        if source.metadata.harness != self.harness || !valid_id(&source.metadata.id) {
            return Err("checkpoint conversation belongs to a different provider layout");
        }
        check_storage(&self.storage, private_home)?;
        check_storage(&source.storage, source.private_home.as_deref())?;
        validate_files(&source.storage, &source.files, operation)?;
        // Validate provider ownership as well as lexical containment. A manifest cannot smuggle
        // configuration/credentials into a provider home under an otherwise safe relative name.
        let selected = (self.select)(&source.storage, &source.metadata.id)
            .map_err(|_| "checkpoint conversation layout is unsupported")?;
        if selected.iter().collect::<std::collections::BTreeSet<_>>()
            != source.files.iter().collect()
        {
            return Err("checkpoint contains files outside the provider conversation");
        }
        crate::execution::platform::private_directory(&self.storage)
            .map_err(|_| "provider conversation storage is unavailable")?;
        // Discover existing files before staging under the storage root: the provider's filename
        // selector must never mistake the staged replacement for an old conversation file.
        let old = (self.select)(&self.storage, &source.metadata.id)
            .map_err(|_| "provider conversation files could not be replaced")?;
        let stage = tempfile::Builder::new()
            .prefix(".conversation-restore-")
            .tempdir_in(&self.storage)
            .map_err(|_| "provider conversation storage is unavailable")?;
        copy_files(source, stage.path(), operation)?;
        operation.check()?;
        for name in old.iter().chain(&source.files) {
            checked_path(&self.storage, name)?;
        }
        for name in old {
            fs::remove_file(checked_path(&self.storage, &name)?)
                .map_err(|_| "provider conversation files could not be replaced")?;
        }
        let installed = (|| {
            for name in &source.files {
                operation.check()?;
                let target = checked_path(&self.storage, name)?;
                crate::execution::platform::private_directory(
                    target.parent().ok_or("invalid conversation name")?,
                )
                .map_err(|_| "provider conversation files could not be restored")?;
                fs::rename(stage.path().join(name), &target)
                    .map_err(|_| "provider conversation files could not be restored")?;
                if let Some(home) = private_home {
                    let mut path = target.as_path();
                    loop {
                        assign_owner(path)
                            .map_err(|_| "provider conversation ownership could not be restored")?;
                        if path == home {
                            break;
                        }
                        path = path.parent().ok_or("invalid conversation path")?;
                    }
                }
            }
            operation.check()
        })();
        if installed.is_err() {
            // A failed install never leaves a partial conversation for a deterministic session ID.
            for name in &source.files {
                if let Ok(path) = checked_path(&self.storage, name) {
                    let _ = fs::remove_file(path);
                }
            }
        }
        installed
    }
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 512
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && id != "."
        && id != ".."
}

/// Bounded, symlink-rejecting filename discovery. The predicate owns provider-specific naming.
pub(crate) fn select_files(
    root: &Path,
    include: impl Fn(&Path) -> bool,
) -> io::Result<Vec<PathBuf>> {
    struct Selection {
        files: Vec<PathBuf>,
        budget: usize,
    }
    fn visit(
        root: &Path,
        relative: &Path,
        include: &dyn Fn(&Path) -> bool,
        selection: &mut Selection,
    ) -> io::Result<()> {
        if relative.components().count() > 64 {
            return Err(io::Error::other(
                "conversation storage is too deeply nested",
            ));
        }
        let entries = match fs::read_dir(root.join(relative)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        for entry in entries {
            selection.budget = selection
                .budget
                .checked_sub(1)
                .ok_or_else(|| io::Error::other("conversation directory is too large"))?;
            let entry = entry?;
            let name = relative.join(entry.file_name());
            let kind = entry.file_type()?;
            if kind.is_dir() {
                visit(root, &name, include, selection)?;
            } else if include(&name) {
                if !kind.is_file() {
                    return Err(io::Error::other("conversation contains a non-file entry"));
                }
                selection.files.push(name);
            }
        }
        Ok(())
    }
    let mut selection = Selection {
        files: Vec::new(),
        budget: MAX_FILES,
    };
    visit(root, Path::new(""), &include, &mut selection)?;
    selection.files.sort();
    Ok(selection.files)
}

#[cfg(all(test, unix))]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod layout_tests;

#[cfg(test)]
pub(crate) mod cancellation_tests;
