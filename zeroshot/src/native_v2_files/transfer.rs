use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use openengine_cluster_protocol::{
    ByteLength, FileEntry, FileEntryKind, FilePath, FILE_TREE_MAGIC, MAX_FILE_ENTRIES,
    MAX_FILE_HEADER_BYTES, MAX_FILE_METADATA_BYTES, MAX_FILE_TRANSFER_BYTES,
};
use sha2::{Digest, Sha256};

use super::{Result, invalid, paths};
#[cfg(windows)]
use crate::execution::platform;

pub(super) struct IndexedEntry {
    pub entry: FileEntry,
    pub input_offset: u64,
}

pub(super) fn write_header(output: &mut dyn Write, entry: &FileEntry) -> Result<usize> {
    let header = serde_json::to_vec(entry)?;
    if header.len() > MAX_FILE_HEADER_BYTES {
        return Err(invalid("file header exceeds limit"));
    }
    output.write_all(&(header.len() as u32).to_be_bytes())?;
    output.write_all(&header)?;
    Ok(4 + header.len())
}

fn copy_body(input: &mut dyn Read, output: &mut dyn Write, length: u64) -> Result<[u8; 32]> {
    let mut hash = Sha256::new();
    let mut left = length;
    let mut buffer = [0_u8; 64 * 1024];
    while left != 0 {
        let count = usize::try_from(left.min(buffer.len() as u64))
            .map_err(|_| invalid("file size overflow"))?;
        input.read_exact(&mut buffer[..count])?;
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
        left -= count as u64;
    }
    Ok(hash.finalize().into())
}

fn advance_transfer(position: &mut u64, bytes: u64) -> Result<()> {
    *position = position
        .checked_add(bytes)
        .filter(|n| *n <= MAX_FILE_TRANSFER_BYTES - 4) // Reserve the final zero header.
        .ok_or_else(|| invalid("file transfer exceeds limit"))?;
    Ok(())
}

/// Validates framing, complete byte counts, hashes, portable uniqueness and parent ownership.
/// Incoming and normalized metadata/transfer sizes are bounded independently before publication.
pub(super) fn copy_validated(
    input: &mut dyn Read,
    output: &mut dyn Write,
    authorize: &mut dyn FnMut(&FileEntry) -> Result<()>,
) -> Result<Vec<IndexedEntry>> {
    let mut magic = [0_u8; 8];
    input.read_exact(&mut magic)?;
    if &magic != FILE_TREE_MAGIC {
        return Err(invalid("unsupported file transfer"));
    }
    output.write_all(FILE_TREE_MAGIC)?;
    let mut entries = Vec::new();
    let mut names = BTreeMap::new();
    let mut aliases = BTreeSet::new();
    let mut metadata = 0_usize;
    let mut position = 8_u64;
    let mut normalized_metadata = 0_usize;
    let mut normalized_position = 8_u64;
    loop {
        let mut length = [0_u8; 4];
        input.read_exact(&mut length)?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 {
            break;
        }
        metadata = metadata
            .checked_add(length)
            .ok_or_else(|| invalid("file metadata overflow"))?;
        if length > MAX_FILE_HEADER_BYTES
            || metadata > MAX_FILE_METADATA_BYTES
            || entries.len() == MAX_FILE_ENTRIES
        {
            return Err(invalid("file tree metadata exceeds limit"));
        }
        let mut header = vec![0_u8; length];
        input.read_exact(&mut header)?;
        let entry: FileEntry =
            serde_json::from_slice(&header).map_err(|_| invalid("invalid file entry"))?;
        let name = entry.path.as_str();
        if entries.is_empty() != name.is_empty() {
            return Err(invalid("file tree requires exactly one leading root"));
        }
        if !name.is_empty() {
            let parent = name.rsplit_once('/').map_or("", |(parent, _)| parent);
            // Exact spelling is required as well as case-insensitive uniqueness.
            if names.get(parent) != Some(&true) {
                return Err(invalid("file tree parent is missing or not a directory"));
            }
        }
        if !aliases.insert(name.to_ascii_lowercase()) {
            return Err(invalid("duplicate or aliased file member"));
        }
        names.insert(
            name.to_owned(),
            matches!(entry.kind, FileEntryKind::Directory),
        );
        authorize(&entry)?;
        // Index the original wire bytes; normalization can shorten or expand a header.
        advance_transfer(&mut position, 4 + length as u64)?;
        let input_offset = position;
        let header_length = write_header(output, &entry)?;
        normalized_metadata = normalized_metadata
            .checked_add(header_length - 4)
            .filter(|n| *n <= MAX_FILE_METADATA_BYTES)
            .ok_or_else(|| invalid("file tree metadata exceeds limit"))?;
        advance_transfer(&mut normalized_position, header_length as u64)?;
        if let FileEntryKind::File { byte_length } = entry.kind {
            copy_validated_body(
                input,
                output,
                byte_length.get(),
                (&mut position, &mut normalized_position),
            )?;
        }
        entries.push(IndexedEntry {
            entry,
            input_offset,
        });
    }
    if entries.is_empty() {
        return Err(invalid("file tree root is missing"));
    }
    if input.read(&mut [0_u8; 1])? != 0 {
        return Err(invalid("trailing file transfer data"));
    }
    output.write_all(&0_u32.to_be_bytes())?;
    Ok(entries)
}

fn copy_validated_body(
    input: &mut dyn Read,
    output: &mut dyn Write,
    length: u64,
    positions: (&mut u64, &mut u64),
) -> Result<()> {
    let (position, normalized_position) = positions;
    let body_length = length
        .checked_add(32)
        .ok_or_else(|| invalid("file transfer exceeds limit"))?;
    advance_transfer(position, body_length)?;
    advance_transfer(normalized_position, body_length)?;
    let digest = copy_body(input, output, length)?;
    let mut expected = [0_u8; 32];
    input.read_exact(&mut expected)?;
    if expected != digest {
        return Err(invalid("file transfer digest mismatch"));
    }
    output.write_all(&expected)
}

pub fn validate_transfer(input: &mut dyn Read) -> Result<Vec<FileEntry>> {
    Ok(
        copy_validated(input, &mut std::io::sink(), &mut |_| Ok(()))?
            .into_iter()
            .map(|entry| entry.entry)
            .collect(),
    )
}

/// Encode one regular file or directory tree; links and special files are rejected.
pub fn pack(source: &Path, output: &mut dyn Write) -> Result<()> {
    pack_with_permissions(source, output).map(|_| ())
}

// Workspace root copies retain executable bits privately without changing the file wire format.
pub(super) fn pack_with_permissions(
    source: &Path,
    output: &mut dyn Write,
) -> Result<Vec<(FilePath, u32)>> {
    let source = std::path::absolute(source)?;
    let _ancestors = paths::guard_ancestors(&source)?;
    let file = paths::open_source(&source)?;
    output.write_all(FILE_TREE_MAGIC)?;
    let mut budget = PackBudget {
        entries: 1,
        metadata: 0,
        bytes: 12,
        executable: Vec::new(),
    };
    pack_entry((&source, file), "", output, &mut budget)?;
    output.write_all(&0_u32.to_be_bytes())?;
    Ok(budget.executable)
}

struct PackBudget {
    entries: usize,
    metadata: usize,
    bytes: u64,
    executable: Vec<(FilePath, u32)>,
}

fn pack_entry(
    source: (&Path, File),
    member: &str,
    output: &mut dyn Write,
    budget: &mut PackBudget,
) -> Result<()> {
    let (source, mut file) = source;
    let _ancestors = paths::guard_ancestors(source)?;
    #[cfg(windows)]
    if platform::file_identity(&file)? != platform::file_identity(&paths::open_source(source)?)? {
        return Err(invalid("source changed during inspection"));
    }
    let metadata = file.metadata()?;
    let path = FilePath::new(member).map_err(invalid)?;
    #[cfg(unix)]
    if metadata.is_file() {
        use std::os::unix::fs::PermissionsExt;
        let executable = metadata.permissions().mode() & 0o111;
        if executable != 0 {
            budget.executable.push((path.clone(), executable));
        }
    }
    let kind = if metadata.is_dir() {
        FileEntryKind::Directory
    } else {
        paths::regular(&file)?;
        FileEntryKind::File {
            byte_length: ByteLength::new(metadata.len())
                .map_err(|_| invalid("file size exceeds limit"))?,
        }
    };
    let header_length = write_header(output, &FileEntry { path, kind })?;
    budget.metadata += header_length - 4;
    budget.bytes = budget
        .bytes
        .checked_add(header_length as u64)
        .and_then(|n| {
            if metadata.is_dir() {
                Some(n)
            } else {
                n.checked_add(metadata.len())?.checked_add(32)
            }
        })
        .filter(|n| *n <= MAX_FILE_TRANSFER_BYTES)
        .ok_or_else(|| invalid("file transfer exceeds limit"))?;
    if budget.metadata > MAX_FILE_METADATA_BYTES {
        return Err(invalid("file tree metadata exceeds limit"));
    }
    if metadata.is_dir() {
        pack_directory((source, &file), member, output, budget)?;
    } else {
        let digest = copy_body(&mut file, output, metadata.len())?;
        if file.read(&mut [0_u8; 1])? != 0 {
            return Err(invalid("source file grew during transfer"));
        }
        output.write_all(&digest)?;
    }
    Ok(())
}

fn pack_directory(
    source: (&Path, &File),
    member: &str,
    output: &mut dyn Write,
    budget: &mut PackBudget,
) -> Result<()> {
    let (source, file) = source;
    // Listing may race source edits, but bytes are opened beneath the already pinned directory.
    // A removed/replaced member either supplies its entire opened file or fails the transfer.
    let mut children = Vec::new();
    for child in std::fs::read_dir(source)? {
        // Reserve discovered siblings before descending so pending directory listings share
        // the same bound instead of each allocating a full tree's worth of names.
        budget.entries += 1;
        if budget.entries > MAX_FILE_ENTRIES {
            return Err(invalid("file tree entry limit exceeded"));
        }
        children.push(child?);
    }
    children.sort_by_key(|child| child.file_name());
    for child in children {
        let name = child.file_name();
        let text = name
            .to_str()
            .ok_or_else(|| invalid("file name must be portable UTF-8"))?;
        let member = if member.is_empty() {
            text.to_owned()
        } else {
            format!("{member}/{text}")
        };
        #[cfg(unix)]
        let opened = paths::open_child(file, &name)?;
        #[cfg(windows)]
        let opened = {
            let opened = paths::open_source(&child.path())?;
            if platform::file_identity(file)?
                != platform::file_identity(&paths::open_source(source)?)?
            {
                return Err(invalid("source directory changed"));
            }
            opened
        };
        pack_entry((&child.path(), opened), &member, output, budget)?;
    }
    Ok(())
}

pub(super) fn project(
    file: &mut File,
    member: &FilePath,
    output: &mut dyn Write,
    authorize: &mut dyn FnMut(&FileEntry) -> Result<()>,
) -> Result<()> {
    file.rewind()?;
    let index = copy_validated(file, &mut std::io::sink(), &mut |_| Ok(()))?;
    if !index.iter().any(|entry| entry.entry.path == *member) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file member was not found",
        ));
    }
    output.write_all(FILE_TREE_MAGIC)?;
    for indexed in index {
        let original = indexed.entry.path.as_str();
        let relative = if original == member.as_str() {
            ""
        } else if member.as_str().is_empty() {
            original
        } else if let Some(relative) = original.strip_prefix(&format!("{}/", member.as_str())) {
            relative
        } else {
            continue;
        };
        authorize(&indexed.entry)?;
        let mut entry = indexed.entry.clone();
        entry.path = FilePath::new(relative).map_err(invalid)?;
        write_header(output, &entry)?;
        if let FileEntryKind::File { byte_length } = entry.kind {
            file.seek(SeekFrom::Start(indexed.input_offset))?;
            let digest = copy_body(file, output, byte_length.get())?;
            output.write_all(&digest)?;
        }
    }
    output.write_all(&0_u32.to_be_bytes())
}

/// Materialize into a private sibling stage and publish a complete selection without clobbering.
/// The caller supplies a host-approved destination. No path in a transfer can name that host path.
pub fn unpack(input: &mut File, destination: &Path) -> Result<()> {
    let destination = std::path::absolute(destination)?;
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("destination requires a parent"))?;
    let parent = paths::Directory::open(parent)?;
    let name = destination
        .file_name()
        .ok_or_else(|| invalid("destination requires a name"))?;
    unpack_at(input, &parent, name)
}

pub(super) fn unpack_at(
    input: &mut File,
    parent: &paths::Directory,
    name: &std::ffi::OsStr,
) -> Result<()> {
    input.rewind()?;
    let index = copy_validated(input, &mut std::io::sink(), &mut |_| Ok(()))?;
    if let FileEntryKind::File { byte_length } = index[0].entry.kind {
        let mut stage = paths::TemporaryFile::new(parent)?;
        input.seek(SeekFrom::Start(index[0].input_offset))?;
        copy_body(input, &mut stage.file, byte_length.get())?;
        return stage.commit_noreplace(name);
    }
    unpack_directory(input, parent, name, index)
}

fn unpack_directory(
    input: &mut File,
    parent: &paths::Directory,
    name: &std::ffi::OsStr,
    index: Vec<IndexedEntry>,
) -> Result<()> {
    let mut stage = StageDirectory::new(parent)?;
    // The private sibling stage is the tree root, so publication renames within one parent.
    // Windows can then retain its parent guards without reopening that parent for write access.
    for indexed in index.into_iter().skip(1) {
        let path = indexed.entry.path.as_str();
        let (parents, leaf) = path.rsplit_once('/').unwrap_or(("", path));
        let mut directory = stage.directory.as_ref().expect("live stage").clone();
        for component in parents.split('/').filter(|part| !part.is_empty()) {
            directory = directory.child(std::ffi::OsStr::new(component))?;
        }
        let leaf = std::ffi::OsStr::new(leaf);
        match indexed.entry.kind {
            FileEntryKind::Directory => {
                directory.create_directory(leaf)?;
            }
            FileEntryKind::File { byte_length } => {
                let mut output = directory.create_file(leaf)?;
                input.seek(SeekFrom::Start(indexed.input_offset))?;
                copy_body(input, &mut output, byte_length.get())?;
                output.sync_all()?;
            }
        }
    }
    // Release the stage's own delete-denying guard; its parent remains pinned through publication.
    stage.directory.take();
    parent.rename_noreplace(&stage.name, parent, name)
}

struct StageDirectory {
    parent: paths::Directory,
    name: std::ffi::OsString,
    directory: Option<paths::Directory>,
}
impl StageDirectory {
    fn new(parent: &paths::Directory) -> Result<Self> {
        let name = std::ffi::OsString::from(format!(".zeroshot-files-{}", uuid::Uuid::now_v7()));
        let directory = parent.create_directory(&name)?;
        Ok(Self {
            parent: parent.clone(),
            name,
            directory: Some(directory),
        })
    }
}
impl Drop for StageDirectory {
    fn drop(&mut self) {
        // Windows must release its stage guard before removing that directory entry. Its parent
        // stays pinned; Unix traversal and unlink remain relative to the original descriptor.
        self.directory.take();
        let _ = self.parent.remove_tree(&self.name);
    }
}

pub(super) fn members(
    file: &mut File,
    member: &FilePath,
    authorize: &mut dyn FnMut(&FileEntry) -> Result<()>,
) -> Result<Vec<FileEntry>> {
    file.rewind()?;
    let index = copy_validated(file, &mut std::io::sink(), &mut |_| Ok(()))?;
    if !index.iter().any(|entry| entry.entry.path == *member) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file member was not found",
        ));
    }
    let mut result = Vec::new();
    for indexed in index {
        let original = indexed.entry.path.as_str();
        let relative = if original == member.as_str() {
            ""
        } else if member.as_str().is_empty() {
            original
        } else if let Some(relative) = original.strip_prefix(&format!("{}/", member.as_str())) {
            relative
        } else {
            continue;
        };
        authorize(&indexed.entry)?;
        let mut entry = indexed.entry.clone();
        entry.path = FilePath::new(relative).map_err(invalid)?;
        result.push(entry);
    }
    Ok(result)
}
