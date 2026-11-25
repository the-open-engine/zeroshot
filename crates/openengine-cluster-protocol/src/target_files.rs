//! Target-relative file addresses and the independent binary transfer contract.
//!
//! These are file-service values, not graph artifacts or OECP methods. A named path denotes one
//! complete file or tree. Members are relative to that immutable selection. Run addresses contain
//! only host-managed publications and cannot be uploaded or removed by a file client.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const TARGET_FILES_KIND: &str = "openengine.target-files/v1";
pub const TARGET_FILES_PATH: &str = "/native-v2/files";
pub const TARGET_FILES_MEDIA_TYPE: &str = "application/vnd.openengine.file-tree.v1";
pub const FILE_TREE_MAGIC: &[u8; 8] = b"OEFILES1";
pub const MAX_FILE_HEADER_BYTES: usize = 4096;
pub const MAX_FILE_ENTRIES: usize = 65_536;
pub const MAX_FILE_METADATA_BYTES: usize = 8 * 1024 * 1024;
/// Compact entries, array separators, echoed selection (bounded by its request), and envelope.
pub const MAX_FILE_RESPONSE_BYTES: usize = MAX_FILE_METADATA_BYTES
    + (MAX_FILE_ENTRIES - 1)
    + MAX_FILE_HEADER_BYTES
    + br#"{"kind":"members","selection":,"entries":[]}"#.len();
pub const MAX_FILE_TRANSFER_BYTES: u64 = 1024 * 1024 * 1024 * 1024;

/// Frozen independent capability. The route and media type above belong to this version.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetFilesDiscovery {
    pub kind: String,
}

/// Portable slash-separated names, with no aliases on supported filesystems. Empty denotes the
/// selected root only; addresses require a nonempty path. Case collisions within a tree are invalid.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub struct FilePath(String);

impl FilePath {
    pub fn new(value: impl Into<String>) -> Result<Self, &'static str> {
        let value = value.into();
        if value.len() > 1024
            || (!value.is_empty()
                && value.split('/').any(|part| {
                    let stem = part
                        .split('.')
                        .next()
                        .unwrap_or_default()
                        .trim_end_matches(' ')
                        .to_ascii_uppercase();
                    part.is_empty()
                        || part.len() > 128
                        || part == "."
                        || part == ".."
                        || part.ends_with(['.', ' '])
                        || part.starts_with(' ')
                        || !part
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_. -".contains(&b))
                        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                        || (stem.len() == 4
                            && (stem.starts_with("COM") || stem.starts_with("LPT"))
                            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
                }))
        {
            return Err("file path must use bounded portable relative names without aliases");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for FilePath {
    type Error = &'static str;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<FilePath> for String {
    fn from(value: FilePath) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileScope {
    User,
    Org,
    Run {
        #[serde(rename = "runId")]
        run_id: crate::RunId,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileAddress {
    pub scope: FileScope,
    pub path: FilePath,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FileSelection {
    pub address: FileAddress,
    pub member: FilePath,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileEntryKind {
    Directory,
    File {
        #[serde(rename = "byteLength")]
        byte_length: crate::ByteLength,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(from = "FileEntryWire", into = "FileEntryWire")]
#[schemars(with = "FileEntryWire")]
pub struct FileEntry {
    pub path: FilePath,
    pub kind: FileEntryKind,
}

#[derive(Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FileEntryWire {
    Directory {
        path: FilePath,
    },
    File {
        path: FilePath,
        #[serde(rename = "byteLength")]
        byte_length: crate::ByteLength,
    },
}
impl From<FileEntryWire> for FileEntry {
    fn from(value: FileEntryWire) -> Self {
        match value {
            FileEntryWire::Directory { path } => Self {
                path,
                kind: FileEntryKind::Directory,
            },
            FileEntryWire::File { path, byte_length } => Self {
                path,
                kind: FileEntryKind::File { byte_length },
            },
        }
    }
}
impl From<FileEntry> for FileEntryWire {
    fn from(value: FileEntry) -> Self {
        match value.kind {
            FileEntryKind::Directory => Self::Directory { path: value.path },
            FileEntryKind::File { byte_length } => Self::File {
                path: value.path,
                byte_length,
            },
        }
    }
}

/// HTTP POST body: big-endian u32 JSON length, this JSON, then a tree for replace only.
/// Tree framing: magic, repeated (big-endian u32 header length, FileEntry JSON, file bytes and
/// raw SHA-256 for files), zero u32, EOF. Parents precede children; root is first and mandatory.
/// Both directions require Content-Length. Receivers validate the complete transfer before publish.
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileRequest {
    Cleanup {
        all: bool,
    },
    List {
        scope: FileScope,
        path: FilePath,
    },
    ListMembers {
        selection: FileSelection,
    },
    Replace {
        address: FileAddress,
        operation: crate::IdempotencyKey,
    },
    Download {
        selection: FileSelection,
    },
    Remove {
        address: FileAddress,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileResponse {
    Cleaned {
        publications: u64,
        #[serde(rename = "checkpointLineages")]
        checkpoint_lineages: u64,
    },
    List {
        paths: Vec<FileAddress>,
    },
    Members {
        selection: FileSelection,
        entries: Vec<FileEntry>,
    },
    Replaced {
        address: FileAddress,
        operation: crate::IdempotencyKey,
    },
    Removed {
        removed: bool,
    },
}
