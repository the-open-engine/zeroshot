use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

pub const MEMORY_SCHEMA_VERSION: u32 = 1;
pub const MAX_LESSONS: usize = 10_000;
pub const MAX_TOMBSTONES: usize = 100_000;
pub const MAX_RECORD_BYTES: usize = 8_192;
pub const MAX_STORE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_INJECTION_BYTES: usize = 16_384;

#[derive(Debug, thiserror::Error)]
pub enum ActiveMemoryError {
    #[error("invalid active memory: {0}")]
    Invalid(String),
    #[error("active memory revision conflict for {id}; expected {expected:?}, found {actual:?}")]
    RevisionConflict {
        id: String,
        expected: Option<u64>,
        actual: Option<u64>,
    },
    #[error("active memory capacity exceeded")]
    Capacity,
    #[error("active memory storage is not a private, host-owned regular file or directory")]
    UnsafeStorage,
    #[error("active memory storage I/O failed")]
    Io(#[from] std::io::Error),
    #[error("active memory database failed")]
    Database(#[from] rusqlite::Error),
    #[error("active memory record encoding failed")]
    Encoding(#[from] serde_json::Error),
}

/// Only costly, recurring lessons are admitted. The rationale and evidence are
/// checked by the postmortem; this schema deliberately has no low-impact tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LessonDraft {
    pub id: String,
    pub scope: LessonScope,
    pub title: String,
    pub recommendation: String,
    pub evidence: LessonEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lesson {
    pub schema_version: u32,
    pub id: String,
    pub revision: u64,
    pub scope: LessonScope,
    pub title: String,
    pub recommendation: String,
    pub evidence: LessonEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LessonScope {
    Repository {
        repository: String,
        files: Vec<String>,
        folders: Vec<String>,
        identifiers: Vec<String>,
        keyword_groups: Vec<Vec<String>>,
    },
    Graph {
        graph_hash: String,
        node_id: String,
        repository: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LessonEvidence {
    pub run_id: String,
    pub node_id: String,
    /// Bounded pointers into run evidence, rather than copied transcripts.
    pub references: Vec<String>,
    pub costly_recurring_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum LessonMutation {
    Upsert {
        expected_revision: Option<u64>,
        draft: Box<LessonDraft>,
    },
    Delete {
        id: String,
        expected_revision: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LessonVersion {
    pub id: String,
    pub revision: u64,
}

/// Absence of a manifest is represented by `None` by the caller, never by an
/// empty manifest. Only successfully supplied cards belong in this list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuppliedMemoryManifest {
    pub lessons: Vec<LessonVersion>,
}

/// Bounded native callback receipt. Tool arguments and results are deliberately absent.
/// A receipt establishes observed tool activity, not success or model compliance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolObservation {
    pub seq: u64,
    pub node_id: String,
    pub execution_id: u64,
    pub timestamp_ms: u64,
    pub tool_name: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ScopeQuery {
    pub repository: Option<String>,
    pub graph_hash: Option<String>,
    pub node_id: Option<String>,
    pub repository_lessons: bool,
    pub graph_lessons: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySnapshot {
    pub generation: u64,
    pub lessons: Vec<Lesson>,
}

impl LessonDraft {
    pub fn validate(&self) -> Result<(), ActiveMemoryError> {
        validate_key("lesson id", &self.id, 96)?;
        validate_text("title", &self.title, 96)?;
        validate_text("recommendation", &self.recommendation, 640)?;
        self.scope.validate()?;
        self.evidence.validate()?;
        if serde_json::to_vec(self)?.len() > MAX_RECORD_BYTES - 128 {
            return Err(ActiveMemoryError::Capacity);
        }
        Ok(())
    }

    pub fn into_lesson(self, revision: u64) -> Lesson {
        Lesson {
            schema_version: MEMORY_SCHEMA_VERSION,
            id: self.id,
            revision,
            scope: self.scope,
            title: self.title,
            recommendation: self.recommendation,
            evidence: self.evidence,
        }
    }
}

impl Lesson {
    pub fn validate(&self) -> Result<(), ActiveMemoryError> {
        if self.schema_version != MEMORY_SCHEMA_VERSION
            || self.revision == 0
            || self.revision > i64::MAX as u64
        {
            return invalid("record version");
        }
        self.draft().validate()?;
        if serde_json::to_vec(self)?.len() > MAX_RECORD_BYTES {
            return Err(ActiveMemoryError::Capacity);
        }
        Ok(())
    }

    pub fn version(&self) -> LessonVersion {
        LessonVersion {
            id: self.id.clone(),
            revision: self.revision,
        }
    }

    pub fn draft(&self) -> LessonDraft {
        LessonDraft {
            id: self.id.clone(),
            scope: self.scope.clone(),
            title: self.title.clone(),
            recommendation: self.recommendation.clone(),
            evidence: self.evidence.clone(),
        }
    }
}

impl LessonScope {
    fn validate(&self) -> Result<(), ActiveMemoryError> {
        match self {
            Self::Repository {
                repository,
                files,
                folders,
                identifiers,
                keyword_groups,
            } => {
                validate_repository(repository)?;
                validate_associations(files, folders, identifiers, keyword_groups)?;
            }
            Self::Graph {
                graph_hash,
                node_id,
                repository,
            } => {
                if graph_hash.len() != 64
                    || !graph_hash
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                {
                    return invalid("authored graph hash must be lowercase SHA-256");
                }
                validate_key("node id", node_id, 128)?;
                if let Some(repository) = repository {
                    validate_repository(repository)?;
                }
            }
        }
        Ok(())
    }

    pub fn matches(&self, query: &ScopeQuery) -> bool {
        match self {
            Self::Repository { repository, .. } => {
                query.repository_lessons && query.repository.as_ref() == Some(repository)
            }
            Self::Graph {
                graph_hash,
                node_id,
                repository,
            } => {
                query.graph_lessons
                    && query.graph_hash.as_ref() == Some(graph_hash)
                    && query.node_id.as_ref().is_none_or(|id| id == node_id)
                    && repository
                        .as_ref()
                        .is_none_or(|repo| query.repository.as_ref() == Some(repo))
            }
        }
    }
}

impl LessonEvidence {
    fn validate(&self) -> Result<(), ActiveMemoryError> {
        validate_key("evidence run id", &self.run_id, 128)?;
        validate_key("evidence node id", &self.node_id, 128)?;
        validate_text(
            "costly recurring rationale",
            &self.costly_recurring_reason,
            512,
        )?;
        if self.references.is_empty() || self.references.len() > 8 {
            return invalid("one to eight evidence references are required");
        }
        unique(&self.references, "evidence references")?;
        for reference in &self.references {
            validate_text("evidence reference", reference, 192)?;
        }
        Ok(())
    }
}

pub(crate) fn validate_key(name: &str, value: &str, max: usize) -> Result<(), ActiveMemoryError> {
    validate_text(name, value, max)?;
    if value.bytes().any(|b| b.is_ascii_whitespace())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:/@+-".contains(&b))
        || matches!(value, "." | "..")
    {
        return invalid(name);
    }
    Ok(())
}

fn validate_repository(value: &str) -> Result<(), ActiveMemoryError> {
    validate_key("repository key", value, 256)?;
    if value == "*" || value.contains("://") || value.contains('@') {
        return invalid("repository key must be a credential-free identity, not a locator");
    }
    Ok(())
}

pub(crate) fn validate_relative_path(value: &str) -> Result<(), ActiveMemoryError> {
    validate_text("root-relative path association", value, 256)?;
    if value.starts_with('/')
        || value.contains(['\\', ':', '*', '?', '[', ']'])
        || value
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return invalid("path association must be a literal root-relative path");
    }
    Ok(())
}

fn validate_text(name: &str, value: &str, max: usize) -> Result<(), ActiveMemoryError> {
    if value.is_empty()
        || value.len() > max
        || value.trim() != value
        || value.chars().any(|c| {
            c.is_control()
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}')
        })
        || value.contains("-----BEGIN ")
        || value.contains("github_pat_")
        || value.contains("ghp_")
        || value.contains("sk-proj-")
    {
        return invalid(name);
    }
    Ok(())
}

fn validate_associations(
    files: &[String],
    folders: &[String],
    identifiers: &[String],
    keyword_groups: &[Vec<String>],
) -> Result<(), ActiveMemoryError> {
    if files.len() + folders.len() + identifiers.len() + keyword_groups.len() == 0 {
        return invalid("repository scope requires an explicit match");
    }
    validate_path_associations(files, folders)?;
    validate_identifiers(identifiers)?;
    validate_keyword_groups(keyword_groups)
}

fn validate_path_associations(
    files: &[String],
    folders: &[String],
) -> Result<(), ActiveMemoryError> {
    if files.len() > 32 || folders.len() > 16 {
        return invalid("scope has too many path associations");
    }
    for path in files.iter().chain(folders) {
        validate_relative_path(path)?;
    }
    unique(files, "file associations")?;
    unique(folders, "folder associations")?;
    Ok(())
}

fn validate_identifiers(identifiers: &[String]) -> Result<(), ActiveMemoryError> {
    if identifiers.len() > 32 {
        return invalid("scope has too many identifier associations");
    }
    unique(identifiers, "identifier associations")?;
    for identifier in identifiers {
        if !(3..=128).contains(&identifier.len())
            || !identifier.bytes().any(|b| b.is_ascii_alphabetic())
            || !identifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return invalid("whole identifier association");
        }
    }
    Ok(())
}

fn validate_keyword_groups(groups: &[Vec<String>]) -> Result<(), ActiveMemoryError> {
    if groups.len() > 8 {
        return invalid("too many keyword groups");
    }
    for group in groups {
        if !(2..=4).contains(&group.len()) {
            return invalid("keywords require two to four conjunctive terms");
        }
        unique(group, "keyword group")?;
        for term in group {
            if !(2..=48).contains(&term.len())
                || !term
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                || matches!(
                    term.as_str(),
                    "the"
                        | "and"
                        | "file"
                        | "files"
                        | "code"
                        | "repo"
                        | "repository"
                        | "read"
                        | "write"
                        | "tool"
                        | "tools"
                )
            {
                return invalid("keyword term");
            }
        }
    }
    Ok(())
}

fn unique(values: &[String], name: &str) -> Result<(), ActiveMemoryError> {
    if values.iter().collect::<BTreeSet<_>>().len() != values.len() {
        return invalid(name);
    }
    Ok(())
}

fn invalid<T>(name: &str) -> Result<T, ActiveMemoryError> {
    Err(ActiveMemoryError::Invalid(name.to_owned()))
}

impl LessonMutation {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Upsert { draft, .. } => &draft.id,
            Self::Delete { id, .. } => id,
        }
    }
}
