use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{HistoricalRun, Result, Review, ReviewTask, Session, files, history, invalid};
use super::super::plan::{self, Role};
use crate::active_memory::{MemorySnapshot, SuppliedMemoryManifest};
use crate::active_memory::source_guard::SourceGuard;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TaskRef {
    pub(super) path: String,
    pub(super) digest: String,
    pub(super) role: Role,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Task {
    instructions: String,
    workspace: std::path::PathBuf,
    context: Value,
    output_contract: Value,
    response_format: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    task_digest: String,
    output: Value,
}

pub(super) fn write(
    directory: &Path,
    run: &HistoricalRun,
    role: Role,
    input: Value,
) -> Result<TaskRef> {
    let mut input = input;
    if let Some(digest) = definition_digest(&input)? {
        input["evidence"]["authored_definition_digest"] = json!(digest);
    }
    let task = Task {
        instructions: format!(
            "{}\n\n{RETROSPECTIVE_INSTRUCTION}",
            plan::specialist_instruction(role)
        ),
        workspace: input["initialization_workspace"]
            .as_str()
            .map(std::path::PathBuf::from)
            .unwrap_or_default(),
        context: input,
        output_contract: plan::output_type(role),
        response_format: json!({"task_digest":"copy from the task reference in session/review metadata","output":"object conforming to output_contract"}),
    };
    let digest = files::digest(&task)?;
    let path = format!("{}-{}-{}.json", run.run_id.as_str(), role.name(), digest);
    files::write(&directory.join(&path), &task)?;
    Ok(TaskRef { path, digest, role })
}

pub(super) fn response(directory: &Path, task_ref: &TaskRef) -> Result<(Value, Value)> {
    if Path::new(&task_ref.path).components().count() != 1 {
        return Err(invalid("unsafe task path"));
    }
    let task: Task = files::read(&directory.join(&task_ref.path))?;
    if files::digest(&task)? != task_ref.digest {
        return Err(invalid("exported task changed"));
    }
    if let Some(actual) = definition_digest(&task.context)? {
        if task.context["evidence"]["authored_definition_digest"].as_str() != Some(actual.as_str())
        {
            return Err(invalid(
                "original authored definition changed after task export",
            ));
        }
    }
    let response: Response =
        files::read(&directory.join(format!("{}.response.json", task_ref.path)))?;
    if response.task_digest != task_ref.digest {
        return Err(invalid(
            "response belongs to a different task or source version",
        ));
    }
    Ok((task.context, response.output))
}

/// A later checkout cannot give an old digest-bound response new source authority.
pub(super) fn response_source<'a>(
    input: &Value,
    inspected: &'a SourceGuard,
    unknown: &'a SourceGuard,
) -> Result<&'a SourceGuard> {
    match input["source_basis"].as_str() {
        Some(reference) if inspected.reference() == Some(reference) => Ok(inspected),
        None => Ok(unknown),
        Some(_) => Err(invalid(
            "source changed since task export; restore it or use a new initialization directory",
        )),
    }
}

fn definition_digest(context: &Value) -> Result<Option<String>> {
    let Some(path) = context["evidence"]["authored_definition_file"].as_str() else {
        return Ok(None);
    };
    let definition: Value = files::read(Path::new(path))?;
    files::digest(&definition).map(Some)
}

#[cfg(test)]
mod tests;

async fn maintenance(
    session: &Session,
    review: &mut Review,
    directory: &Path,
    assignment: Assignment<'_>,
) -> Result<()> {
    let run = assignment.run;
    let role = assignment.role;
    let loaded = history::open(&session.state_root, &run.run_id).await?;
    let evidence = loaded.evidence().await?;
    let snapshot = assignment.snapshot;
    let supplied = SuppliedMemoryManifest {
        lessons: snapshot
            .lessons
            .iter()
            .map(crate::active_memory::Lesson::version)
            .collect(),
    };
    let source = SourceGuard::capture(&run.workspace, &snapshot.lessons).await;
    let definition_file = loaded.export_definition(directory)?;
    let context = loaded
        .proposal_context(
            session.lesson_context(run, &snapshot),
            &evidence,
            Some(supplied),
        )
        .with_definition_file(Some(&definition_file))
        .with_source(&source);
    for mut input in context
        .inputs(role)
        .map_err(|()| invalid("cannot fit a complete maintenance card"))?
    {
        input["initialization_source_digest"] = json!(review.source_digest);
        input["initialization_workspace"] = json!(run.workspace);
        let task = write(directory, run, role, input)?;
        review.tasks.push(ReviewTask {
            run: run.clone(),
            task,
        });
    }
    Ok(())
}

const RETROSPECTIVE_INSTRUCTION: &str = concat!(
    "Retrospective initialization: evidence is the supplied HISTORICAL main run, not a new execution. Main-run citations name that frozen context. ",
    "Read current source in workspace before source-dependent judgments. The complete exact authored graph and original nonsecret runtime are at evidence.authored_definition_file. ",
    "Graph lessons remain tied to that exact hash; never migrate them to another graph version. The final maintenance pass checks tracked and nonignored source; ",
    "ignored/generated files cannot be current authority. Do not execute git commands. Write only your response file; do not alter source or memory state. ",
    "Native tool receipts are unavailable; use observed historical completions and transcripts.",
);

pub(super) async fn discovery(
    session: &Session,
    run: &HistoricalRun,
    directory: &Path,
) -> Result<Vec<TaskRef>> {
    let loaded = history::discovery(session, run).await?;
    let definition_file = loaded.export_definition(directory)?;
    let source = SourceGuard::capture(&run.workspace, &[]).await;
    let context = loaded
        .context(session, run)
        .with_source(&source)
        .with_definition_file(Some(&definition_file));
    let mut exported = Vec::new();
    for role in [Role::Graph, Role::Repository] {
        if !context.role_enabled(role) {
            continue;
        }
        for mut input in context
            .inputs(role)
            .map_err(|()| invalid("cannot fit discovery context"))?
        {
            input["initialization_workspace"] = json!(run.workspace);
            exported.push(write(directory, run, role, input)?);
        }
    }
    Ok(exported)
}

pub(super) async fn final_reviews(
    session: &Session,
    review: &mut Review,
    directory: &Path,
    runs: &[HistoricalRun],
) -> Result<()> {
    let mut assigned = BTreeSet::new();
    for role in [Role::GraphMaintenance, Role::RepositoryMaintenance] {
        for run in runs.iter().filter(|run| assignment_enabled(run, role)) {
            let lessons = review
                .snapshot
                .lessons
                .iter()
                .filter(|lesson| {
                    role.accepts_scope(&lesson.scope)
                        && run.accepts(&lesson.scope)
                        && !assigned.contains(&lesson.id)
                })
                .cloned()
                .collect::<Vec<_>>();
            if lessons.is_empty() {
                continue;
            }
            assigned.extend(lessons.iter().map(|lesson| lesson.id.clone()));
            let snapshot = MemorySnapshot {
                generation: review.snapshot.generation,
                lessons,
            };
            maintenance(
                session,
                review,
                directory,
                Assignment {
                    run,
                    role,
                    snapshot,
                },
            )
            .await?;
        }
    }
    Ok(())
}

struct Assignment<'a> {
    run: &'a HistoricalRun,
    role: Role,
    snapshot: MemorySnapshot,
}

fn assignment_enabled(run: &HistoricalRun, role: Role) -> bool {
    match role {
        Role::GraphMaintenance => run.graph_enabled,
        Role::RepositoryMaintenance => run.repository_enabled,
        _ => false,
    }
}
