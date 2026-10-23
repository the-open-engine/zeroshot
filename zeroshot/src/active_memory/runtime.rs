//! Host composition for local and direct-target Active Memory.
//!
//! A host chooses the persistent namespace. Workers receive only scoped cards from a
//! frozen run snapshot, never the database path or mutation authority.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openengine_cluster_protocol::RunId;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::active_memory::{
    ActiveMemoryError, ActiveMemoryStore, LessonVersion, MemoryIndex, MemorySnapshot,
    RetrievalQuery, RetrievalResult, ScopeQuery, SharedActiveMemoryStore, SuppliedMemoryManifest,
    ToolObservation,
};
use crate::active_memory::hooks::{ActiveMemoryHooks, ActiveMemorySource, HookQuery};
use crate::active_memory::postmortem::{
    BuiltinPostmortem, BuiltinPostmortemConfig, PostmortemError, PostmortemReport,
    PostmortemRequest, RunPostmortem, SuppliedLessonSource,
};
use crate::native_v2_contract::AdmittedRun;
use crate::native_v2_runner::{
    AuxiliaryRunnerFactory, NativeNodeRunner, NodeHandle, NodeRunRequest, NodeRunner,
    NodeRunnerError,
};

const MAX_QUERY_BYTES: usize = 65_536;
const MAX_QUERY_FIELDS: usize = 1024;
const MAX_SESSIONS: usize = 16_384;
const MAX_TOOL_OBSERVATIONS: usize = 512;

pub struct MemoryPreparation<'a> {
    pub admitted: &'a AdmittedRun,
    pub workspace: &'a Path,
    pub storage: Option<&'a Path>,
}

/// Prepared before adapters are built, so every lane shares one immutable index.
pub struct PreparedActiveMemory {
    pub hooks: Arc<ActiveMemoryHooks>,
    store: SharedActiveMemoryStore,
    snapshot: MemorySnapshot,
    source: Arc<RunMemorySource>,
    repository: Option<String>,
    graph_hash: String,
}

impl PreparedActiveMemory {
    pub fn prepare(request: MemoryPreparation<'_>) -> Result<Option<Self>, ActiveMemoryError> {
        let config = request.admitted.runtime.active_memory();
        let Some(storage) = request.storage.filter(|_| config.enabled()) else {
            return Ok(None);
        };
        if config.budget_bytes == 0
            || config.budget_bytes as usize > crate::active_memory::MAX_INJECTION_BYTES
        {
            return Err(ActiveMemoryError::Invalid("context budget".into()));
        }
        // Bootstrap/config paths are host-owned. Still reject accidental placement in the checkout.
        crate::native_v2_local::validate_workspace_state(request.workspace, storage)
            .map_err(|_| ActiveMemoryError::UnsafeStorage)?;
        let repository = Some(repository_key(request.admitted, request.workspace));
        let graph_hash = authored_graph_hash(request.admitted)?;
        let store = ActiveMemoryStore::open(storage)?;
        let snapshot = store.snapshot(&ScopeQuery {
            repository: repository.clone(),
            graph_hash: Some(graph_hash.clone()),
            node_id: None,
            repository_lessons: config.repository,
            graph_lessons: config.graph,
        })?;
        let source = Arc::new(RunMemorySource {
            index: MemoryIndex::new(snapshot.clone())?,
            workspace: request.workspace.to_path_buf(),
            budget: config.budget_bytes as usize,
            repository_enabled: config.repository,
            graph_enabled: config.graph,
            nodes: request
                .admitted
                .runtime
                .nodes()
                .keys()
                .map(|node| node.as_str().to_owned())
                .collect(),
            state: Mutex::new(ExposureState::default()),
        });
        let hooks = Arc::new(ActiveMemoryHooks::new(source.clone())?);
        Ok(Some(Self {
            hooks,
            store: Arc::new(tokio::sync::Mutex::new(store)),
            snapshot,
            source,
            repository,
            graph_hash,
        }))
    }

    /// The global agents reuse the same admitted provider adapters with isolated sessions.
    pub fn wrap(self, admitted: &AdmittedRun, runner: NativeNodeRunner) -> Arc<dyn NodeRunner> {
        let factory = runner.auxiliary_factory();
        let postmortem: Option<Arc<dyn RunPostmortem>> =
            if admitted.runtime.global_agents().postmortem.enabled {
                Some(Arc::new(BuiltinPostmortem::new(BuiltinPostmortemConfig {
                    store: self.store,
                    factory: factory.clone(),
                    snapshot: self.snapshot,
                    supplied: self.source.clone(),
                    repository: self.repository,
                    graph_hash: self.graph_hash,
                    workspace: self.source.workspace.clone(),
                })))
            } else {
                None
            };
        Arc::new(MemoryNodeRunner {
            inner: Arc::new(runner),
            postmortem,
            factory: Some(factory),
            _hooks: Some(self.hooks),
        })
    }
}

struct MemoryNodeRunner {
    inner: Arc<dyn NodeRunner>,
    postmortem: Option<Arc<dyn RunPostmortem>>,
    factory: Option<Arc<dyn AuxiliaryRunnerFactory>>,
    _hooks: Option<Arc<ActiveMemoryHooks>>,
}

/// Keeps global work at the host while the main runner uses the capsule transport.
pub fn attach_postmortem(
    inner: Arc<dyn NodeRunner>,
    postmortem: Option<Arc<dyn RunPostmortem>>,
) -> Arc<dyn NodeRunner> {
    Arc::new(MemoryNodeRunner {
        inner,
        postmortem,
        factory: None,
        _hooks: None,
    })
}

pub fn unavailable_memory(inner: Arc<dyn NodeRunner>) -> Arc<dyn NodeRunner> {
    attach_postmortem(inner, Some(Arc::new(UnavailableMemory)))
}

struct UnavailableMemory;

#[async_trait]
impl RunPostmortem for UnavailableMemory {
    async fn run(
        &self,
        _request: PostmortemRequest,
        _cancellation: tokio::sync::watch::Receiver<bool>,
    ) -> Result<PostmortemReport, PostmortemError> {
        Err(PostmortemError::Unavailable(
            "Active Memory storage or hook initialization was unavailable",
        ))
    }
}

#[async_trait]
impl NodeRunner for MemoryNodeRunner {
    fn postmortem(&self) -> Option<Arc<dyn RunPostmortem>> {
        self.postmortem.clone()
    }
    async fn start(&self, request: NodeRunRequest) -> Result<NodeHandle, NodeRunnerError> {
        self.inner.start(request).await
    }
    async fn close_run(&self, run_id: &RunId) {
        let close_global = async {
            if let Some(factory) = &self.factory {
                factory.close_run(run_id).await;
            }
        };
        tokio::join!(self.inner.close_run(run_id), close_global);
    }
}

#[derive(Default)]
struct SessionExposure {
    versions: BTreeSet<LessonVersion>,
    bytes: usize,
}

#[derive(Default)]
struct ExposureState {
    sessions: BTreeMap<String, SessionExposure>,
    supplied: BTreeSet<LessonVersion>,
    unavailable: bool,
    observation_seq: u64,
    observations: VecDeque<ToolObservation>,
}

/// Matching takes one short in-memory lock; no filesystem or model work occurs on this path.
struct RunMemorySource {
    index: MemoryIndex,
    workspace: PathBuf,
    budget: usize,
    repository_enabled: bool,
    graph_enabled: bool,
    nodes: BTreeSet<String>,
    state: Mutex<ExposureState>,
}

impl RunMemorySource {
    fn observe_tool(&self, query: &HookQuery, retrieval: &RetrievalQuery) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let Some(seq) = state.observation_seq.checked_add(1) else {
            state.unavailable = true;
            return;
        };
        state.observation_seq = seq;
        if state.observations.len() == MAX_TOOL_OBSERVATIONS {
            // Preserve the beginning and end of a long run, with a fixed memory bound.
            state.observations.remove(MAX_TOOL_OBSERVATIONS / 2);
        }
        state.observations.push_back(ToolObservation {
            seq,
            node_id: query.node_id.clone(),
            execution_id: query.execution_id,
            timestamp_ms: crate::native_v2_runner::current_timestamp().get(),
            tool_name: crate::native_v2_runner::bounded_log_text(query.tool_name.clone(), 96),
            paths: observed_paths(retrieval),
        });
    }

    fn select(
        &self,
        session: &str,
        select: impl FnOnce(usize, &BTreeSet<LessonVersion>) -> RetrievalResult,
    ) -> String {
        let Ok(mut state) = self.state.lock() else {
            return String::new();
        };
        if !state.sessions.contains_key(session) && state.sessions.len() >= MAX_SESSIONS {
            state.unavailable = true;
            return String::new();
        }
        let exposure = state.sessions.entry(session.to_owned()).or_default();
        let result = select(
            self.budget.saturating_sub(exposure.bytes),
            &exposure.versions,
        );
        exposure.bytes += result.bytes_used;
        let versions: Vec<_> = result.cards.into_iter().map(|card| card.version).collect();
        exposure.versions.extend(versions.iter().cloned());
        state.supplied.extend(versions);
        result.context
    }
}

impl ActiveMemorySource for RunMemorySource {
    fn tool_context(&self, query: &HookQuery) -> String {
        if !self.nodes.contains(&query.node_id) {
            return String::new();
        }
        let mut retrieval = RetrievalQuery {
            workspace: self.workspace.clone(),
            tool_cwd: Some(query.cwd.clone()),
            ..Default::default()
        };
        let input = decoded_input(&query.tool_input);
        // Keep the primary shell command at the beginning for lexical `cd` resolution.
        if let Some(command) = input
            .get("cmd")
            .or_else(|| input.get("command"))
            .and_then(Value::as_str)
        {
            append_bounded(&mut retrieval.text, command);
        }
        let mut fields = 0;
        collect_tool_input(&input, &mut retrieval, &mut fields, 0);
        append_bounded(&mut retrieval.text, &query.tool_name);
        self.observe_tool(query, &retrieval);
        if !self.repository_enabled {
            return String::new();
        }
        self.select(&query.session_key, |budget, delivered| {
            self.index.retrieve(&retrieval, budget, delivered)
        })
    }

    fn graph_context(&self, node_id: &str, session_key: &str) -> String {
        if !self.graph_enabled || !self.nodes.contains(node_id) {
            return String::new();
        }
        self.select(session_key, |budget, delivered| {
            self.index.graph_guidance(node_id, budget, delivered)
        })
    }

    fn unavailable(&self, _reason: &str) {
        if let Ok(mut state) = self.state.lock() {
            state.unavailable = true;
        }
    }
}

fn decoded_input(input: &Value) -> Cow<'_, Value> {
    if let Some(text) = input.as_str().filter(|text| text.len() <= MAX_QUERY_BYTES) {
        if let Ok(decoded) = serde_json::from_str::<Value>(text) {
            return Cow::Owned(decoded);
        }
    }
    Cow::Borrowed(input)
}

impl SuppliedLessonSource for RunMemorySource {
    fn tool_observations(&self) -> Vec<ToolObservation> {
        self.state
            .lock()
            .map(|state| state.observations.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn manifest(&self) -> Option<SuppliedMemoryManifest> {
        let state = self.state.lock().ok()?;
        // A broken delivery path cannot prove the complete supplied set. Maintenance abstains.
        if state.unavailable {
            return None;
        }
        Some(SuppliedMemoryManifest {
            lessons: state.supplied.iter().cloned().collect(),
        })
    }
}

fn observed_paths(query: &RetrievalQuery) -> Vec<String> {
    let cwd = query.tool_cwd.as_deref().unwrap_or(&query.workspace);
    query
        .paths
        .iter()
        .filter_map(|path| crate::active_memory::normalize_path(&query.workspace, cwd, path))
        .filter(|path| path.len() <= 256)
        .take(4)
        .collect()
}

fn collect_tool_input(value: &Value, query: &mut RetrievalQuery, fields: &mut usize, depth: usize) {
    if *fields >= MAX_QUERY_FIELDS || depth > 8 || query.text.len() >= MAX_QUERY_BYTES {
        return;
    }
    *fields += 1;
    match value {
        Value::String(text) => append_bounded(&mut query.text, text),
        Value::Array(values) => {
            collect_array_fields(values, query, fields, depth);
        }
        Value::Object(values) => {
            collect_object_fields(values, query, fields, depth);
        }
        _ => {}
    }
}

fn collect_array_fields(
    values: &[Value],
    query: &mut RetrievalQuery,
    fields: &mut usize,
    depth: usize,
) {
    for value in values.iter().take(MAX_QUERY_FIELDS - *fields) {
        collect_tool_input(value, query, fields, depth + 1);
    }
}

fn collect_object_fields(
    values: &serde_json::Map<String, Value>,
    query: &mut RetrievalQuery,
    fields: &mut usize,
    depth: usize,
) {
    for (key, value) in values.iter().take(MAX_QUERY_FIELDS - *fields) {
        collect_tool_field(key, value, query);
        if !is_primary_command(key, value, depth) {
            collect_tool_input(value, query, fields, depth + 1);
        }
    }
}

fn is_primary_command(key: &str, value: &Value, depth: usize) -> bool {
    depth == 0 && matches!(key, "cmd" | "command") && value.is_string()
}

fn collect_tool_field(key: &str, value: &Value, query: &mut RetrievalQuery) {
    let Some(path) = value.as_str().filter(|path| path.len() <= 2048) else {
        return;
    };
    if is_file_field(key) && query.paths.len() < 128 {
        query.paths.push(path.to_owned());
    }
    if matches!(key, "workdir" | "cwd" | "workingDirectory") {
        let path = Path::new(path);
        query.tool_cwd = Some(if path.is_absolute() {
            path.to_path_buf()
        } else {
            query
                .tool_cwd
                .as_deref()
                .unwrap_or(&query.workspace)
                .join(path)
        });
    }
}

fn is_file_field(key: &str) -> bool {
    matches!(key, "file_path" | "filePath" | "path" | "file" | "filename")
}

fn append_bounded(output: &mut String, text: &str) {
    if output.len() >= MAX_QUERY_BYTES {
        return;
    }
    output.push(' ');
    let mut end = text.len().min(MAX_QUERY_BYTES - output.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    output.push_str(&text[..end]);
}

/// Stable admitted source identity crosses local and Docker placement without storing locators.
pub fn repository_key(admitted: &AdmittedRun, workspace: &Path) -> String {
    repository_key_for_source(admitted.source.as_ref(), workspace)
}

pub(super) fn repository_key_for_source(
    source: Option<&openengine_cluster_protocol::ResolvedSource>,
    workspace: &Path,
) -> String {
    let identity = match source {
        Some(source) => match source.remote.as_ref() {
            Some(remote) => remote_identity(&remote.url),
            None => return repository_key_for_repository(&source.repository),
        },
        None => format!("local:{}", workspace.to_string_lossy()),
    };
    identity_key(&identity)
}

pub(super) fn repository_key_for_repository(
    repository: &openengine_cluster_protocol::SourceRepositoryId,
) -> String {
    identity_key(&format!("source:{}", repository.as_str()))
}

pub(super) fn repository_key_for_remote(
    remote: &openengine_cluster_protocol::GitRemoteUrl,
) -> String {
    identity_key(&remote_identity(remote))
}

fn identity_key(identity: &str) -> String {
    format!("repo:{:x}", Sha256::digest(identity.as_bytes()))
}

fn remote_identity(remote: &openengine_cluster_protocol::GitRemoteUrl) -> String {
    // Derive from the checkout locator through the existing provider registry. Caller-authored
    // forge metadata is not evidence of which repository was actually checked out.
    match crate::source_code_provider::adapters::core::forge_identity(remote) {
        Ok(Some(forge)) => format!(
            "forge:{}/{}",
            forge.provider.as_str(),
            forge.repository.as_str()
        ),
        _ => format!("remote:{}", remote.as_str()),
    }
}

pub fn authored_graph_hash(admitted: &AdmittedRun) -> Result<String, ActiveMemoryError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&admitted.graph)?)
    ))
}

#[cfg(test)]
mod tests;
