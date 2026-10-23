use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use super::{
    ActiveMemoryError, Lesson, LessonScope, LessonVersion, MAX_INJECTION_BYTES, MAX_LESSONS,
    MAX_STORE_BYTES, MemorySnapshot,
};

const PREFIX: &str = "<active_memory>\nPreviously observed guidance; check current source. Authored instructions, permissions and output contracts take precedence.\n";
const SUFFIX: &str = "</active_memory>\n";
const MAX_QUERY_BYTES: usize = 65_536;
const MAX_QUERY_PATHS: usize = 128;

#[derive(Debug, Clone, Default)]
pub struct RetrievalQuery {
    pub workspace: PathBuf,
    pub tool_cwd: Option<PathBuf>,
    /// Native file path fields are preferred over shell-command heuristics.
    pub paths: Vec<String>,
    /// Observable tool arguments or public text, never required hidden reasoning.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetrievedCard {
    pub version: LessonVersion,
    pub score: u16,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetrievalResult {
    pub cards: Vec<RetrievedCard>,
    pub context: String,
    pub bytes_used: usize,
}

/// Resident inverted indexes. Retrieval does no filesystem, process, model or
/// network work; its input and output work are bounded independently of tool size.
pub struct MemoryIndex {
    lessons: Vec<Lesson>,
    files: BTreeMap<String, Vec<usize>>,
    folders: BTreeMap<String, Vec<usize>>,
    identifiers: BTreeMap<String, Vec<usize>>,
    keywords: BTreeMap<String, Vec<(usize, usize)>>,
    keyword_lengths: BTreeMap<(usize, usize), usize>,
    nodes: BTreeMap<String, Vec<usize>>,
}

impl MemoryIndex {
    pub fn new(mut snapshot: MemorySnapshot) -> Result<Self, ActiveMemoryError> {
        validate_snapshot(&snapshot)?;
        snapshot.lessons.sort_by(|a, b| a.id.cmp(&b.id));
        let mut index = Self {
            lessons: snapshot.lessons,
            files: BTreeMap::new(),
            folders: BTreeMap::new(),
            identifiers: BTreeMap::new(),
            keywords: BTreeMap::new(),
            keyword_lengths: BTreeMap::new(),
            nodes: BTreeMap::new(),
        };
        for position in 0..index.lessons.len() {
            index.index_lesson(position);
        }
        Ok(index)
    }

    fn index_lesson(&mut self, position: usize) {
        match &self.lessons[position].scope {
            LessonScope::Repository {
                files,
                folders,
                identifiers,
                keyword_groups,
                ..
            } => {
                add_associations(&mut self.files, files, position);
                add_associations(&mut self.folders, folders, position);
                add_associations(&mut self.identifiers, identifiers, position);
                for (group_id, group) in keyword_groups.iter().enumerate() {
                    self.keyword_lengths
                        .insert((position, group_id), group.len());
                    for term in group {
                        self.keywords
                            .entry(term.clone())
                            .or_default()
                            .push((position, group_id));
                    }
                }
            }
            LessonScope::Graph { node_id, .. } => {
                self.nodes
                    .entry(node_id.clone())
                    .or_default()
                    .push(position);
            }
        }
    }

    /// Scores are the strongest independent match, never a count of repeated
    /// words: exact file 1000, folder 800, whole identifier 700, conjunction 500.
    pub fn retrieve(
        &self,
        query: &RetrievalQuery,
        budget: usize,
        delivered: &BTreeSet<LessonVersion>,
    ) -> RetrievalResult {
        let mut scores = BTreeMap::new();
        let text = bounded_text(&query.text);
        self.score_paths(&mut scores, query_paths(query, text));
        let mut whole = BTreeSet::new();
        let mut topics = BTreeSet::new();
        collect_tokens(text, &mut whole, &mut topics);
        for path in query.paths.iter().take(MAX_QUERY_PATHS) {
            collect_tokens(bounded_text(path), &mut whole, &mut topics);
        }
        for identifier in whole {
            add_scores(&mut scores, self.identifiers.get(&identifier), 700);
        }
        self.score_keywords(&mut scores, topics);
        self.render(scores, budget, delivered)
    }

    fn score_paths(&self, scores: &mut BTreeMap<usize, u16>, paths: BTreeSet<String>) {
        for path in paths {
            add_scores(scores, self.files.get(&path), 1000);
            let mut prefix = path.as_str();
            loop {
                add_scores(scores, self.folders.get(prefix), 800);
                let Some((parent, _)) = prefix.rsplit_once('/') else {
                    break;
                };
                prefix = parent;
            }
        }
    }

    fn score_keywords(&self, scores: &mut BTreeMap<usize, u16>, topics: BTreeSet<String>) {
        let mut group_matches = BTreeMap::<(usize, usize), usize>::new();
        for topic in topics {
            if let Some(groups) = self.keywords.get(&topic) {
                for group in groups {
                    *group_matches.entry(*group).or_default() += 1;
                }
            }
        }
        for (group, count) in group_matches {
            if self.keyword_lengths.get(&group) == Some(&count) {
                score(scores, group.0, 500);
            }
        }
    }

    /// Graph lessons are selected only for their actual authored agent node.
    /// The snapshot already enforces graph hash and optional repository scope.
    pub fn graph_guidance(
        &self,
        node_id: &str,
        budget: usize,
        delivered: &BTreeSet<LessonVersion>,
    ) -> RetrievalResult {
        let mut scores = BTreeMap::new();
        add_scores(&mut scores, self.nodes.get(node_id), 1000);
        self.render(scores, budget, delivered)
    }

    fn render(
        &self,
        scores: BTreeMap<usize, u16>,
        budget: usize,
        delivered: &BTreeSet<LessonVersion>,
    ) -> RetrievalResult {
        let budget = budget.min(MAX_INJECTION_BYTES);
        if budget <= PREFIX.len() + SUFFIX.len() {
            return RetrievalResult::default();
        }
        let mut ranked: Vec<_> = scores.into_iter().collect();
        ranked.sort_by(|(a, a_score), (b, b_score)| {
            b_score
                .cmp(a_score)
                .then_with(|| self.lessons[*a].id.cmp(&self.lessons[*b].id))
        });
        let mut result = RetrievalResult::default();
        let mut used = PREFIX.len() + SUFFIX.len();
        for (position, score) in ranked {
            let lesson = &self.lessons[position];
            let version = lesson.version();
            if delivered.contains(&version) {
                continue;
            }
            let text = format!(
                "[{}@{}] {}\n{}\n",
                lesson.id, lesson.revision, lesson.title, lesson.recommendation
            );
            if text.len() > budget - used {
                continue;
            }
            used += text.len();
            result.cards.push(RetrievedCard {
                version,
                score,
                text,
            });
        }
        if !result.cards.is_empty() {
            result.context.push_str(PREFIX);
            for card in &result.cards {
                result.context.push_str(&card.text);
            }
            result.context.push_str(SUFFIX);
            result.bytes_used = result.context.len();
        }
        result
    }
}

fn validate_snapshot(snapshot: &MemorySnapshot) -> Result<(), ActiveMemoryError> {
    if snapshot.lessons.len() > MAX_LESSONS {
        return Err(ActiveMemoryError::Capacity);
    }
    let mut ids = BTreeSet::new();
    let mut bytes = 0usize;
    for lesson in &snapshot.lessons {
        lesson.validate()?;
        if !ids.insert(&lesson.id) {
            return Err(ActiveMemoryError::Invalid("duplicate snapshot id".into()));
        }
        bytes = bytes
            .checked_add(serde_json::to_vec(lesson)?.len())
            .ok_or(ActiveMemoryError::Capacity)?;
    }
    if bytes > MAX_STORE_BYTES {
        return Err(ActiveMemoryError::Capacity);
    }
    Ok(())
}

fn add_associations(index: &mut BTreeMap<String, Vec<usize>>, values: &[String], position: usize) {
    for value in values {
        index.entry(value.clone()).or_default().push(position);
    }
}

fn add_scores(scores: &mut BTreeMap<usize, u16>, positions: Option<&Vec<usize>>, value: u16) {
    if let Some(positions) = positions {
        for position in positions {
            score(scores, *position, value);
        }
    }
}

fn score(scores: &mut BTreeMap<usize, u16>, position: usize, value: u16) {
    let current = scores.entry(position).or_default();
    *current = (*current).max(value);
}

fn bounded_text(text: &str) -> &str {
    let mut end = text.len().min(MAX_QUERY_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn query_paths(query: &RetrievalQuery, text: &str) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    let base = query.tool_cwd.as_deref().unwrap_or(&query.workspace);
    for path in query.paths.iter().take(MAX_QUERY_PATHS) {
        if let Some(path) = normalize_path(&query.workspace, base, path) {
            paths.insert(path);
        }
    }
    // This is deliberately a bounded shell heuristic. Arbitrary scripts, shell
    // variables and generated filenames require native path metadata to match.
    let mut cwd = base.to_path_buf();
    for segment in text
        .split([';', '\n'])
        .flat_map(|segment| segment.split("&&"))
    {
        let words: Vec<_> = segment
            .split(|c: char| c.is_whitespace() || "\"'`(){}[],|&=<>".contains(c))
            .filter(|word| !word.is_empty())
            .collect();
        if words.first() == Some(&"cd") {
            if let Some(directory) = words.get(1) {
                if let Some(next) = absolute_path(&query.workspace, &cwd, directory) {
                    cwd = next;
                }
            }
        }
        for word in words {
            if let Some(path) = normalize_path(&query.workspace, &cwd, word) {
                paths.insert(path);
            }
        }
    }
    paths
}

/// Lexical normalization intentionally avoids stat/canonicalize on the hot path.
/// An escaping path produces no match, rather than a path in another repository.
pub fn normalize_path(workspace: &Path, cwd: &Path, raw: &str) -> Option<String> {
    if raw.len() > 4096
        || raw.chars().any(char::is_control)
        || (!cfg!(windows) && raw.contains('\\'))
    {
        return None;
    }
    let root = lexical_absolute(workspace)?;
    let absolute = absolute_path(&root, cwd, raw)?;
    let relative = absolute.strip_prefix(&root).ok()?;
    let result = relative.to_str()?.replace(std::path::MAIN_SEPARATOR, "/");
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

fn absolute_path(workspace: &Path, cwd: &Path, raw: &str) -> Option<PathBuf> {
    if raw.is_empty() || raw.contains(['*', '?', '$', '~']) {
        return None;
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        lexical_absolute(path)
    } else {
        let base = if cwd.is_absolute() {
            cwd.to_path_buf()
        } else {
            workspace.join(cwd)
        };
        lexical_absolute(&base.join(path))
    }
}

fn lexical_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !result.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            other => result.push(other.as_os_str()),
        }
    }
    Some(result)
}

fn collect_tokens(text: &str, whole: &mut BTreeSet<String>, topics: &mut BTreeSet<String>) {
    for token in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if token.is_empty() || token.len() > 128 {
            continue;
        }
        whole.insert(token.to_owned());
        topics.insert(token.to_ascii_lowercase());
        for part in token.split('_').filter(|part| !part.is_empty()) {
            collect_camel_parts(part, topics);
        }
    }
}

fn collect_camel_parts(part: &str, topics: &mut BTreeSet<String>) {
    let bytes = part.as_bytes();
    let mut start = 0;
    for position in 1..bytes.len() {
        if is_camel_boundary(bytes, position) {
            topics.insert(part[start..position].to_ascii_lowercase());
            start = position;
        }
    }
    topics.insert(part[start..].to_ascii_lowercase());
}

fn is_camel_boundary(bytes: &[u8], position: usize) -> bool {
    let previous = bytes[position - 1];
    let current = bytes[position];
    let next_lower = bytes.get(position + 1).is_some_and(u8::is_ascii_lowercase);
    current.is_ascii_uppercase()
        && (previous.is_ascii_lowercase()
            || previous.is_ascii_digit()
            || (previous.is_ascii_uppercase() && next_lower))
}
