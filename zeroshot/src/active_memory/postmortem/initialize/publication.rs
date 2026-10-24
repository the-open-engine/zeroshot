use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use super::{Result, Review, ReviewTask, Session, files, history, invalid, tasks};
use crate::active_memory::{Lesson, LessonMutation, MemorySnapshot, SuppliedMemoryManifest};

pub(super) async fn validate(
    session: &Session,
    review: &Review,
    directory: &Path,
) -> Result<(Vec<LessonMutation>, usize)> {
    let mut source_files = BTreeMap::new();
    let mut reconciliation = Reconciliation {
        pending: review
            .snapshot
            .lessons
            .iter()
            .map(|lesson| (lesson.id.clone(), lesson))
            .collect(),
        new_ids: &review.new_ids,
        mutations: Vec::new(),
        deferred: 0,
    };
    for entry in &review.tasks {
        let (output, changes) = validate_task(session, entry, directory).await?;
        if !source_files.contains_key(&entry.run.workspace) {
            source_files.insert(
                entry.run.workspace.clone(),
                files::source_files(&entry.run.workspace).await?,
            );
        }
        reconciliation.decisions(
            output,
            changes,
            SourceAuthority {
                workspace: &entry.run.workspace,
                files: &source_files[&entry.run.workspace],
            },
        )?;
    }
    if !reconciliation.pending.is_empty() {
        return Err(invalid(
            "maintenance has not reconciled every staged and existing lesson",
        ));
    }
    Ok((reconciliation.mutations, reconciliation.deferred))
}

async fn validate_task(
    session: &Session,
    entry: &ReviewTask,
    directory: &Path,
) -> Result<(Value, Vec<LessonMutation>)> {
    let (input, output) = tasks::response(directory, &entry.task)?;
    let loaded = history::open(&session.state_root, &entry.run.run_id).await?;
    let evidence = loaded.evidence().await?;
    let supplied = input["supplied_lessons"]
        .as_object()
        .ok_or_else(|| invalid("missing maintenance manifest"))?;
    let snapshot = MemorySnapshot {
        generation: 0,
        lessons: supplied
            .values()
            .cloned()
            .map(serde_json::from_value)
            .collect::<std::result::Result<Vec<_>, _>>()?,
    };
    let manifest = SuppliedMemoryManifest {
        lessons: snapshot
            .lessons
            .iter()
            .map(crate::active_memory::Lesson::version)
            .collect(),
    };
    let context = loaded.proposal_context(
        session.lesson_context(&entry.run, &snapshot),
        &evidence,
        Some(manifest),
    );
    let validated = context
        .mutations(entry.task.role, &input, output.clone())
        .map_err(|reason| {
            invalid(format!(
                "invalid maintenance result: {}: {reason}",
                entry.task.path
            ))
        })?;
    Ok((output, validated))
}

struct Reconciliation<'a> {
    pending: BTreeMap<String, &'a Lesson>,
    new_ids: &'a BTreeSet<String>,
    mutations: Vec<LessonMutation>,
    deferred: usize,
}

impl Reconciliation<'_> {
    fn decisions(
        &mut self,
        output: Value,
        changes: Vec<LessonMutation>,
        authority: SourceAuthority<'_>,
    ) -> Result<()> {
        let mut changes = changes
            .into_iter()
            .map(|mutation| (mutation.id().to_owned(), mutation))
            .collect::<BTreeMap<_, _>>();
        for decision in output["decisions"]
            .as_array()
            .ok_or_else(|| invalid("missing decisions"))?
        {
            let id = decision["id"]
                .as_str()
                .ok_or_else(|| invalid("missing decision ID"))?;
            let original = self
                .pending
                .remove(id)
                .ok_or_else(|| invalid("duplicate or foreign maintenance coverage"))?;
            authority.verify_citations(decision)?;
            self.decision(original, decision, changes.remove(id))?;
        }
        Ok(())
    }

    fn decision(
        &mut self,
        original: &Lesson,
        decision: &Value,
        change: Option<LessonMutation>,
    ) -> Result<()> {
        let is_new = self.new_ids.contains(&original.id);
        match decision["action"].as_str() {
            Some("defer") => self.deferred += 1,
            Some("keep") if is_new => self.mutations.push(LessonMutation::Upsert {
                expected_revision: None,
                draft: Box::new(original.draft()),
            }),
            Some("adapt") => {
                self.mutations.push(adaptation(original, is_new, change)?);
            }
            Some("retire") => self.mutations.extend(retirement(is_new, change)?),
            Some("keep") => {}
            _ => return Err(invalid("invalid maintenance action")),
        }
        Ok(())
    }
}

struct SourceAuthority<'a> {
    workspace: &'a Path,
    files: &'a BTreeSet<String>,
}

impl SourceAuthority<'_> {
    fn verify_citations(&self, decision: &Value) -> Result<()> {
        for reference in decision["references"]
            .as_array()
            .ok_or_else(|| invalid("missing references"))?
        {
            if let Some(path) = reference
                .as_str()
                .and_then(|reference| reference.strip_prefix("repository:"))
            {
                self.verify_path(path)?;
            }
        }
        Ok(())
    }
    fn verify_path(&self, path: &str) -> Result<()> {
        let canonical = self.workspace.join(path).canonicalize()?;
        let relative = canonical
            .strip_prefix(self.workspace)
            .map_err(|_| invalid("current authority escaped the checkout"))?;
        let relative = relative
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if !self.files.contains(path) || !self.files.contains(&relative) {
            return Err(invalid(
                "current authority and canonical symlink targets must be tracked or nonignored source",
            ));
        }
        Ok(())
    }
}

fn adaptation(
    original: &Lesson,
    is_new: bool,
    change: Option<LessonMutation>,
) -> Result<LessonMutation> {
    let Some(LessonMutation::Upsert { draft, .. }) = change else {
        return Err(invalid("missing validated adaptation"));
    };
    Ok(LessonMutation::Upsert {
        draft,
        expected_revision: (!is_new).then_some(original.revision),
    })
}

fn retirement(is_new: bool, change: Option<LessonMutation>) -> Result<Option<LessonMutation>> {
    if is_new {
        Ok(None)
    } else {
        change
            .map(Some)
            .ok_or_else(|| invalid("missing validated retirement"))
    }
}
