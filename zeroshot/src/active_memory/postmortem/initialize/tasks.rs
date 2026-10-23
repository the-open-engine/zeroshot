use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{HistoricalRun, Result, Review, ReviewTask, Session, files, history, invalid};
use super::super::plan::{self, Role};
use crate::active_memory::{MemorySnapshot, SuppliedMemoryManifest};

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
    let response: Response =
        files::read(&directory.join(format!("{}.response.json", task_ref.path)))?;
    if response.task_digest != task_ref.digest {
        return Err(invalid(
            "response belongs to a different task or source version",
        ));
    }
    Ok((task.context, response.output))
}

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
    let context = loaded.proposal_context(
        session.lesson_context(run, &snapshot),
        &evidence,
        Some(supplied),
    );
    for mut input in context
        .inputs(role)
        .map_err(|()| invalid("cannot fit a complete maintenance card"))?
    {
        input["initialization_source_digest"] = json!(review.source_digest);
        input["initialization_workspace"] = json!(run.workspace);
        input["initialization_graph_file"] = json!(loaded.export_graph(directory)?);
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
    "Read current source in workspace before source-dependent judgments. The complete exact authored graph is at context.initialization_graph_file if the evidence graph is truncated. ",
    "Graph lessons remain tied to that exact hash; never migrate them to another graph version. The final maintenance pass checks tracked and nonignored source; ",
    "ignored/generated files cannot be current authority. Do not execute git commands. Write only your response file; do not alter source or memory state. ",
    "Native tool receipts are unavailable, so exploration without durable qualifying failure evidence cannot be admitted.",
);

pub(super) async fn discovery(
    session: &Session,
    run: &HistoricalRun,
    directory: &Path,
) -> Result<Vec<TaskRef>> {
    let loaded = history::discovery(session, run).await?;
    let context = loaded.context(session, run);
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
            input["initialization_graph_file"] = json!(loaded.export_graph(directory)?);
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
