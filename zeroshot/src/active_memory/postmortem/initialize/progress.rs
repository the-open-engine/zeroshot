use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Result, Review, Selection, Session, files, load, publish, review, selected_runs};

#[derive(Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
enum Progress {
    Discovery,
    Maintenance { selection_digest: String },
    Complete { report: Value },
}

pub(crate) async fn init(
    state: PathBuf,
    directory: &Path,
    filters: super::Ignore,
) -> Result<Value> {
    let directory = std::path::absolute(directory)?;
    super::preparation::ensure(state, &filters, &directory).await?;
    let session = load(&directory)?;
    session.filters.validate_resume(&filters)?;
    let _lock = super::preparation::SessionLock::acquire(&directory)?;
    let progress =
        files::optional(&directory.join("progress.json"))?.unwrap_or(Progress::Discovery);
    advance(&session, &directory, progress).await
}

async fn advance(session: &Session, directory: &Path, progress: Progress) -> Result<Value> {
    if let Progress::Complete { report } = progress {
        return complete(session, directory, report);
    }
    if let Some(report) = publish::recover(session, directory)? {
        return complete(session, directory, report);
    }
    match progress {
        Progress::Discovery => discovery(session, directory).await,
        Progress::Maintenance { selection_digest } => {
            maintenance(session, directory, &selection_digest).await
        }
        Progress::Complete { .. } => unreachable!(),
    }
}

async fn discovery(session: &Session, directory: &Path) -> Result<Value> {
    let selection: Selection = files::read(&directory.join("selection.json"))?;
    if selection
        .runs
        .iter()
        .any(|run| run.reason.trim().is_empty() || run.reason == "unclassified")
    {
        let mut report: Value = files::read(&directory.join("inventory.json"))?;
        report["phase"] = json!("discovery");
        report["next_action"] = json!("select_runs");
        return Ok(report);
    }
    let runs = selected_runs(session, directory)?;
    if runs.is_empty() {
        return complete(
            session,
            directory,
            json!({"published_mutations":0,"reviewed_lessons":0,
        "deferred":0,"versions":[],"reason":"no_selected_runs"}),
        );
    }
    let missing = pending_discovery(directory, &runs)?;
    if !missing.is_empty() {
        return Ok(
            json!({"phase":"discovery","next_action":"run_discovery","pending_tasks":missing}),
        );
    }
    prepare_maintenance(session, directory, None).await
}

fn pending_discovery(directory: &Path, runs: &[super::HistoricalRun]) -> Result<Vec<Value>> {
    let mut missing = Vec::new();
    for run in runs {
        for task in &run.tasks {
            if !response_exists(directory, task)? {
                missing.push(json!({"run_id":run.run_id,"task":task}));
            }
        }
    }
    Ok(missing)
}

async fn prepare_maintenance(
    _session: &Session,
    directory: &Path,
    reason: Option<&str>,
) -> Result<Value> {
    let mut report = if reason.is_some() {
        super::refresh_review(_session, directory).await?
    } else {
        review(directory).await?
    };
    let selection: Selection = files::read(&directory.join("selection.json"))?;
    files::write(
        &directory.join("progress.json"),
        &Progress::Maintenance {
            selection_digest: files::digest(&selection)?,
        },
    )?;
    report["phase"] = json!("maintenance");
    report["next_action"] = json!("run_maintenance");
    report["refresh_reason"] = json!(reason);
    Ok(report)
}

async fn maintenance(session: &Session, directory: &Path, selection_digest: &str) -> Result<Value> {
    let selection: Selection = files::read(&directory.join("selection.json"))?;
    if files::digest(&selection)? != selection_digest {
        files::write(&directory.join("progress.json"), &Progress::Discovery)?;
        return discovery(session, directory).await;
    }
    let review: Review = files::read(&directory.join("review.json"))?;
    if !review_current(session, &review).await? {
        return prepare_maintenance(session, directory, Some("source_or_lessons_changed")).await;
    }
    let missing = pending_maintenance(directory, &review)?;
    if !missing.is_empty() {
        return Ok(
            json!({"phase":"maintenance","next_action":"run_maintenance","maintenance_tasks":missing}),
        );
    }
    publish_or_refresh(session, directory).await
}

async fn publish_or_refresh(session: &Session, directory: &Path) -> Result<Value> {
    match super::commit(directory).await {
        Ok(report) => complete(session, directory, report),
        Err(super::InitializationError::Memory(
            crate::active_memory::ActiveMemoryError::RevisionConflict { .. },
        )) => prepare_maintenance(session, directory, Some("lesson_revision_conflict")).await,
        Err(error) => Err(error),
    }
}

fn pending_maintenance(directory: &Path, review: &Review) -> Result<Vec<Value>> {
    let mut missing = Vec::new();
    for entry in &review.tasks {
        if !response_exists(directory, &entry.task)? {
            missing.push(json!({"run":entry.run,"task":entry.task}));
        }
    }
    Ok(missing)
}

async fn review_current(session: &Session, review: &Review) -> Result<bool> {
    if super::checkouts::sources_digest(session, &review.workspaces).await? != review.source_digest
    {
        return Ok(false);
    }
    let store = session.store()?;
    for lesson in &review.snapshot.lessons {
        let expected = (!review.new_ids.contains(&lesson.id)).then_some(lesson.revision);
        if store
            .record(&lesson.id)?
            .map(|record| record.version.revision)
            != expected
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn response_exists(directory: &Path, task: &super::tasks::TaskRef) -> Result<bool> {
    Ok(directory
        .join(format!("{}.response.json", task.path))
        .try_exists()?)
}

fn complete(session: &Session, directory: &Path, mut report: Value) -> Result<Value> {
    let inventory: Value = files::read(&directory.join("inventory.json"))?;
    report["excluded"] = inventory["excluded"].clone();
    report["filters"] = inventory["filters"].clone();
    report["phase"] = json!("complete");
    report["next_action"] = json!("done");
    files::write(
        &directory.join("progress.json"),
        &Progress::Complete {
            report: report.clone(),
        },
    )?;
    publish::acknowledge(session, directory);
    Ok(report)
}
