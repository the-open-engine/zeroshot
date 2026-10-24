use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::evidence::FrozenEvidence;
use super::plan::Role;
use super::{BuiltinPostmortemConfig, MainRunFacts};
use crate::active_memory::{
    Lesson, LessonDraft, LessonEvidence, LessonMutation, LessonScope, MAX_STORE_BYTES,
    SuppliedMemoryManifest,
};

/// Scope and frozen cards used by validation; this carries no store or provider capability.
#[derive(Clone, Copy)]
pub(super) struct LessonContext<'a> {
    pub snapshot: &'a crate::active_memory::MemorySnapshot,
    pub repository: &'a Option<String>,
    pub graph_hash: &'a str,
    pub workspace: &'a std::path::Path,
}

impl<'a> From<&'a BuiltinPostmortemConfig> for LessonContext<'a> {
    fn from(config: &'a BuiltinPostmortemConfig) -> Self {
        Self {
            snapshot: &config.snapshot,
            repository: &config.repository,
            graph_hash: &config.graph_hash,
            workspace: &config.workspace,
        }
    }
}

/// Only validation stages and host-owned field names enter diagnostics, never proposal text.
#[derive(Debug, thiserror::Error)]
pub(super) enum ProposalRejection {
    #[error("invalid output shape or serialized size")]
    Output,
    #[error("incomplete or invalid maintenance reconciliation")]
    Maintenance,
    #[error("disabled scope or unknown executed node")]
    Node,
    #[error("invalid evidence references")]
    References,
    #[error("cited references do not prove the required main-run cost for this node")]
    Cost,
    #[error("current repository source does not verify the proposed associations")]
    Source,
    #[error("invalid lesson scope")]
    Scope,
    #[error("invalid lesson card: {0}")]
    Card(crate::active_memory::ActiveMemoryError),
}

pub(super) struct ProposalContext<'a> {
    config: LessonContext<'a>,
    request: MainRunFacts<'a>,
    evidence: &'a FrozenEvidence,
    supplied: Option<SuppliedMemoryManifest>,
    supplied_lessons: BTreeMap<String, Lesson>,
    actual_nodes: BTreeSet<String>,
}

impl<'a> ProposalContext<'a> {
    pub(super) fn new(
        config: impl Into<LessonContext<'a>>,
        request: impl Into<MainRunFacts<'a>>,
        evidence: &'a FrozenEvidence,
        supplied: Option<SuppliedMemoryManifest>,
    ) -> Self {
        let config = config.into();
        let request = request.into();
        let versions = supplied
            .as_ref()
            .map(|manifest| manifest.lessons.iter().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let supplied_lessons = config
            .snapshot
            .lessons
            .iter()
            .filter(|lesson| versions.contains(&lesson.version()))
            .map(|lesson| (lesson.id.clone(), lesson.clone()))
            .collect();
        let actual_nodes = request
            .snapshot
            .executions
            .values()
            .filter(|node| {
                matches!(
                    request.admitted.runtime.nodes().get(&node.reference.node),
                    Some(crate::native_v2_contract::NodeRuntimeBinding::Agent { .. })
                )
            })
            .map(|node| node.reference.node.as_str().to_owned())
            .collect();
        Self {
            config,
            request,
            evidence,
            supplied,
            supplied_lessons,
            actual_nodes,
        }
    }

    pub(super) fn role_enabled(&self, role: Role) -> bool {
        let settings = self.request.admitted.runtime.active_memory();
        match role {
            Role::GraphMaintenance | Role::Graph => settings.graph,
            Role::RepositoryMaintenance | Role::Repository => {
                settings.repository && self.config.repository.is_some()
            }
        }
    }

    pub(super) fn inputs(&self, role: Role) -> Result<Vec<Value>, ()> {
        let mut input = super::context_budget::base(json!({
            "role":role.name(), "repository":self.config.repository, "graph_hash":self.config.graph_hash,
            "scope_controls":{"repository":self.request.admitted.runtime.active_memory().repository,
                "graph":self.request.admitted.runtime.active_memory().graph},
            "manifest_known":self.supplied.is_some(),
            "supplied_lessons":{},
            "supplied_manifest_total":self.supplied_lessons.values().filter(|lesson| role.accepts_scope(&lesson.scope)).count(),
            "maintenance_batch":null,
            "known_lesson_count":self.config.snapshot.lessons.iter().filter(|lesson| role.accepts_scope(&lesson.scope)).count(),
            "existing_lessons_truncated":false,
            "existing_lessons":[],
            "evidence":self.evidence.document,
        }));
        if role.is_maintenance() {
            self.maintenance_inputs(role, input)
        } else {
            self.append_existing(role, &mut input);
            Ok(vec![input])
        }
    }

    fn maintenance_inputs(&self, role: Role, template: Value) -> Result<Vec<Value>, ()> {
        if self
            .supplied
            .as_ref()
            .is_some_and(|manifest| manifest.lessons.len() != self.supplied_lessons.len())
        {
            return Err(());
        }
        let mut batches = Vec::new();
        let mut input = template.clone();
        for (id, lesson) in self
            .supplied_lessons
            .iter()
            .filter(|(_, lesson)| role.accepts_scope(&lesson.scope))
        {
            let lesson = serde_json::to_value(lesson).map_err(|_| ())?;
            if !super::context_budget::add_object(
                &mut input,
                "supplied_lessons",
                id,
                lesson.clone(),
            ) {
                if input["supplied_lessons"].as_object().ok_or(())?.is_empty() {
                    return Err(());
                }
                batches.push(input);
                input = template.clone();
                if !super::context_budget::add_object(&mut input, "supplied_lessons", id, lesson) {
                    return Err(());
                }
            }
        }
        batches.push(input);
        let total = batches.len();
        for (index, input) in batches.iter_mut().enumerate() {
            input["maintenance_batch"] = json!({"index":index + 1,"total":total});
        }
        Ok(batches)
    }

    fn append_existing(&self, role: Role, input: &mut Value) {
        let mut included = 0;
        for lesson in self
            .config
            .snapshot
            .lessons
            .iter()
            .filter(|lesson| role.accepts_scope(&lesson.scope))
        {
            let card = json!({"id":lesson.id,"revision":lesson.revision,"scope":lesson.scope,
                "title":lesson.title,"recommendation":lesson.recommendation});
            if !super::context_budget::add_array(input, "existing_lessons", card) {
                break;
            }
            included += 1;
        }
        input["existing_lessons_truncated"] =
            json!(included < input["known_lesson_count"].as_u64().unwrap_or_default() as usize);
    }

    pub(super) fn mutations(
        &self,
        role: Role,
        input: &Value,
        output: Value,
    ) -> Result<Vec<LessonMutation>, ProposalRejection> {
        if serde_json::to_vec(&output)
            .map_err(|_| ProposalRejection::Output)?
            .len()
            > MAX_STORE_BYTES
        {
            return Err(ProposalRejection::Output);
        }
        if role.is_maintenance() {
            return self.maintenance_mutations(role, input, output);
        }
        let output: DiscoveryOutput =
            serde_json::from_value(output).map_err(|_| ProposalRejection::Output)?;
        let mutations = output
            .lessons
            .into_iter()
            .map(|candidate| self.discovery(role, candidate))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(mutations
            .into_iter()
            .filter(|mutation| {
                let LessonMutation::Upsert { draft, .. } = mutation else {
                    return false;
                };
                !self.config.snapshot.lessons.iter().any(|lesson| {
                    lesson.id == draft.id
                        || (canonical_scope(&lesson.scope) == draft.scope
                            && normalize(&lesson.recommendation)
                                == normalize(&draft.recommendation))
                })
            })
            .collect())
    }

    fn maintenance_mutations(
        &self,
        role: Role,
        input: &Value,
        output: Value,
    ) -> Result<Vec<LessonMutation>, ProposalRejection> {
        let output: MaintenanceOutput =
            serde_json::from_value(output).map_err(|_| ProposalRejection::Output)?;
        let mut pending = input["supplied_lessons"]
            .as_object()
            .ok_or(ProposalRejection::Maintenance)?
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut mutations = Vec::new();
        for decision in output.decisions {
            if !pending.remove(&decision.id) {
                return Err(ProposalRejection::Maintenance);
            }
            if let Some(mutation) = self.maintenance(role, decision)? {
                mutations.push(mutation);
            }
        }
        if !pending.is_empty() {
            return Err(ProposalRejection::Maintenance);
        }
        Ok(mutations)
    }

    fn discovery(
        &self,
        role: Role,
        candidate: Candidate,
    ) -> Result<LessonMutation, ProposalRejection> {
        if !self.role_enabled(role) || !self.actual_nodes.contains(&candidate.node_id) {
            return Err(ProposalRejection::Node);
        }
        self.references(&candidate.references)
            .map_err(|()| ProposalRejection::References)?;
        if !self
            .evidence
            .impact
            .episodes
            .get(&candidate.node_id)
            .is_some_and(|episode| episode.allows(role, &candidate.references))
        {
            return Err(ProposalRejection::Cost);
        }
        if role == Role::Repository {
            self.repository_discovery_source(&candidate)
                .map_err(|()| ProposalRejection::Source)?;
        }
        let scope = canonical_scope(
            &self
                .discovery_scope(role, &candidate)
                .map_err(|()| ProposalRejection::Scope)?,
        );
        let encoded = serde_json::to_vec(&(&scope, normalize(&candidate.recommendation)))
            .map_err(|_| ProposalRejection::Output)?;
        let digest = Sha256::digest(encoded);
        let draft = LessonDraft {
            id: format!("lesson-{:x}", digest),
            scope,
            title: candidate.title,
            recommendation: candidate.recommendation,
            evidence: LessonEvidence {
                run_id: self.request.run_id.as_str().to_owned(),
                node_id: candidate.node_id,
                references: candidate.references,
                costly_recurring_reason: candidate.costly_recurring_reason,
            },
        };
        validate_advice(&draft).map_err(ProposalRejection::Card)?;
        Ok(LessonMutation::Upsert {
            expected_revision: None,
            draft: Box::new(draft),
        })
    }

    fn discovery_scope(&self, role: Role, candidate: &Candidate) -> Result<LessonScope, ()> {
        Ok(match role {
            Role::Repository => LessonScope::Repository {
                repository: self.config.repository.clone().ok_or(())?,
                files: candidate.files.clone(),
                folders: candidate.folders.clone(),
                identifiers: candidate.identifiers.clone(),
                keyword_groups: candidate.keyword_groups.clone(),
            },
            Role::Graph => {
                if candidate.repository_specific && self.config.repository.is_none() {
                    return Err(());
                }
                LessonScope::Graph {
                    graph_hash: self.config.graph_hash.to_owned(),
                    node_id: candidate.node_id.clone(),
                    repository: if candidate.repository_specific {
                        self.config.repository.clone()
                    } else {
                        None
                    },
                }
            }
            Role::GraphMaintenance | Role::RepositoryMaintenance => return Err(()),
        })
    }

    fn maintenance(
        &self,
        role: Role,
        decision: MaintenanceDecision,
    ) -> Result<Option<LessonMutation>, ProposalRejection> {
        if self.supplied.is_none() {
            return Err(ProposalRejection::Maintenance);
        }
        let original = self
            .supplied_lessons
            .get(&decision.id)
            .ok_or(ProposalRejection::Maintenance)?;
        if decision
            .legacy_revision
            .is_some_and(|revision| revision != original.revision)
            || !role.accepts_scope(&original.scope)
            || !self.scope_enabled(&original.scope)
        {
            return Err(ProposalRejection::Maintenance);
        }
        self.maintenance_action(role, original, decision)
    }

    fn maintenance_action(
        &self,
        role: Role,
        original: &Lesson,
        decision: MaintenanceDecision,
    ) -> Result<Option<LessonMutation>, ProposalRejection> {
        match decision.action.as_str() {
            "defer" => Ok(None),
            "keep" => {
                validate_advice(&original.draft()).map_err(ProposalRejection::Card)?;
                self.current_maintenance_evidence(original, &decision)
                    .map_err(|()| ProposalRejection::Maintenance)?;
                Ok(None)
            }
            "retire" => {
                self.maintenance_change_evidence(original, &decision)?;
                Ok(Some(LessonMutation::Delete {
                    id: original.id.clone(),
                    expected_revision: original.revision,
                }))
            }
            "adapt" => self.adaptation(role, original, decision).map(Some),
            _ => Err(ProposalRejection::Maintenance),
        }
    }

    fn maintenance_change_evidence(
        &self,
        original: &Lesson,
        decision: &MaintenanceDecision,
    ) -> Result<(), ProposalRejection> {
        self.references(&decision.references)
            .map_err(|()| ProposalRejection::References)?;
        self.current_maintenance_evidence(original, decision)
            .map_err(|()| ProposalRejection::Maintenance)
    }

    fn adaptation(
        &self,
        role: Role,
        original: &Lesson,
        decision: MaintenanceDecision,
    ) -> Result<LessonMutation, ProposalRejection> {
        self.maintenance_change_evidence(original, &decision)?;
        let mut draft = original.draft();
        draft.title = decision.title;
        draft.recommendation = decision.recommendation;
        if let LessonScope::Repository {
            files,
            folders,
            identifiers,
            keyword_groups,
            ..
        } = &mut draft.scope
        {
            *files = decision.files;
            *folders = decision.folders;
            *identifiers = decision.identifiers;
            *keyword_groups = decision.keyword_groups;
        }
        draft.evidence = LessonEvidence {
            run_id: self.request.run_id.as_str().to_owned(),
            node_id: role.name().to_owned(),
            references: decision.references,
            costly_recurring_reason: decision.reason,
        };
        draft.scope = canonical_scope(&draft.scope);
        validate_advice(&draft).map_err(ProposalRejection::Card)?;
        Ok(LessonMutation::Upsert {
            expected_revision: Some(original.revision),
            draft: Box::new(draft),
        })
    }

    fn references(&self, references: &[String]) -> Result<(), ()> {
        if references.is_empty()
            || references.len() > 8
            || !references
                .iter()
                .any(|reference| self.evidence.references.contains(reference))
        {
            return Err(());
        }
        for reference in references {
            self.reference(reference)?;
        }
        Ok(())
    }

    fn current_maintenance_evidence(
        &self,
        original: &Lesson,
        decision: &MaintenanceDecision,
    ) -> Result<(), ()> {
        maintenance_reason(&decision.reason)?;
        if !(1..=8).contains(&decision.references.len()) {
            return Err(());
        }
        if !self.has_current_source(&original.scope, &decision.references) {
            return Err(());
        }
        for reference in &decision.references {
            self.reference(reference)?;
        }
        Ok(())
    }

    fn has_current_source(&self, scope: &LessonScope, references: &[String]) -> bool {
        match scope {
            LessonScope::Repository { .. } => references
                .iter()
                .any(|reference| reference.starts_with("repository:")),
            LessonScope::Graph {
                node_id,
                graph_hash,
                ..
            } => {
                graph_hash == self.config.graph_hash
                    && references
                        .iter()
                        .any(|reference| reference == &format!("graph:{node_id}"))
            }
        }
    }

    fn reference(&self, reference: &str) -> Result<(), ()> {
        if self.evidence.references.contains(reference) {
            return Ok(());
        }
        if let Some(node) = reference.strip_prefix("graph:") {
            let node = openengine_cluster_protocol::NodeName::new(node).map_err(|_| ())?;
            return match self.request.admitted.runtime.nodes().get(&node) {
                Some(crate::native_v2_contract::NodeRuntimeBinding::Agent { .. }) => Ok(()),
                _ => Err(()),
            };
        }
        self.repository_reference(reference)
    }

    fn repository_reference(&self, reference: &str) -> Result<(), ()> {
        self.repository_file(reference).map(|_| ())
    }

    fn repository_file(&self, reference: &str) -> Result<std::fs::File, ()> {
        let relative = reference.strip_prefix("repository:").ok_or(())?;
        super::source_file::open(self.config.workspace, relative).map_err(|_| ())
    }

    fn repository_discovery_source(&self, candidate: &Candidate) -> Result<(), ()> {
        for reference in &candidate.references {
            let Some(relative) = reference.strip_prefix("repository:") else {
                continue;
            };
            let file = self.repository_file(reference)?;
            if candidate.files.iter().any(|file| file == relative)
                || candidate
                    .folders
                    .iter()
                    .any(|folder| std::path::Path::new(relative).starts_with(folder))
                || super::source_file::has_identifier(file, &candidate.identifiers)
            {
                return Ok(());
            }
        }
        Err(())
    }

    fn scope_enabled(&self, scope: &LessonScope) -> bool {
        let settings = self.request.admitted.runtime.active_memory();
        match scope {
            LessonScope::Repository { .. } => settings.repository,
            LessonScope::Graph { .. } => settings.graph,
        }
    }
}

fn maintenance_reason(reason: &str) -> Result<(), ()> {
    if reason.trim().is_empty() || reason.len() > 512 || reason.chars().any(char::is_control) {
        return Err(());
    }
    Ok(())
}

// Admission-only policy: legacy records stay readable until scoped maintenance adapts them.
fn validate_advice(draft: &LessonDraft) -> Result<(), crate::active_memory::ActiveMemoryError> {
    draft.validate()?;
    let LessonScope::Repository { files, .. } = &draft.scope else {
        return Ok(());
    };
    let mut paths = files.iter().map(String::as_str).chain(
        draft
            .evidence
            .references
            .iter()
            .filter_map(|reference| reference.strip_prefix("repository:")),
    );
    if paths.any(|path| {
        source_position(&draft.title, path) || source_position(&draft.recommendation, path)
    }) {
        return Err(crate::active_memory::ActiveMemoryError::Invalid(
            "source line positions in repository advice; use stable paths and identifiers"
                .to_owned(),
        ));
    }
    Ok(())
}

fn source_position(text: &str, path: &str) -> bool {
    let plain_path = (path.contains('/') && std::path::Path::new(path).extension().is_some())
        .then(|| format!("{path}:"));
    [Some(format!("./{path}:")), plain_path]
        .into_iter()
        .flatten()
        .any(|prefix| {
            text.match_indices(prefix.as_str()).any(|(offset, _)| {
                let before = &text[..offset];
                source_reference_boundary(before)
                    && !non_source_context(before)
                    && text
                        .as_bytes()
                        .get(offset + prefix.len())
                        .is_some_and(u8::is_ascii_digit)
            })
        })
}

fn source_reference_boundary(before: &str) -> bool {
    before
        .chars()
        .next_back()
        .is_none_or(|c| c.is_whitespace() || "([`'\"*".contains(c))
}

fn non_source_context(before: &str) -> bool {
    let token = before
        .rsplit(char::is_whitespace)
        .next()
        .unwrap_or_default();
    let clause = before.rsplit([';', '\n']).next().unwrap_or_default();
    let clause = clause.rsplit_once(". ").map_or(clause, |(_, tail)| tail);
    token.contains("://")
        || clause.split_whitespace().rev().take(4).any(|word| {
            let word = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
            ["image", "docker", "podman", "tag"]
                .into_iter()
                .any(|kind| word.eq_ignore_ascii_case(kind))
        })
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn canonical_scope(scope: &LessonScope) -> LessonScope {
    let mut scope = scope.clone();
    if let LessonScope::Repository {
        files,
        folders,
        identifiers,
        keyword_groups,
        ..
    } = &mut scope
    {
        files.sort();
        files.dedup();
        folders.sort();
        folders.dedup();
        identifiers.sort();
        identifiers.dedup();
        for group in keyword_groups.iter_mut() {
            group.sort();
            group.dedup();
        }
        keyword_groups.sort();
        keyword_groups.dedup();
    }
    scope
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoveryOutput {
    lessons: Vec<Candidate>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    title: String,
    recommendation: String,
    node_id: String,
    files: Vec<String>,
    folders: Vec<String>,
    identifiers: Vec<String>,
    keyword_groups: Vec<Vec<String>>,
    references: Vec<String>,
    costly_recurring_reason: String,
    repository_specific: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceOutput {
    decisions: Vec<MaintenanceDecision>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceDecision {
    id: String,
    // Prior initialization response files echoed the revision; preserve their precondition.
    #[serde(default, rename = "revision")]
    legacy_revision: Option<u64>,
    action: String,
    title: String,
    recommendation: String,
    files: Vec<String>,
    folders: Vec<String>,
    identifiers: Vec<String>,
    keyword_groups: Vec<Vec<String>>,
    references: Vec<String>,
    reason: String,
}
