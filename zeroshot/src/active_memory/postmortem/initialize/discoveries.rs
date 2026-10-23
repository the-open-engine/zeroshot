use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{HistoricalRun, Result, Session, files, history, invalid, tasks};
use crate::active_memory::{ActiveMemoryStore, Lesson, LessonMutation, MemorySnapshot};

pub(super) async fn collect(
    session: &Session,
    runs: &[HistoricalRun],
    mut snapshot: MemorySnapshot,
    directory: &Path,
) -> Result<(MemorySnapshot, BTreeSet<String>)> {
    let store = session.store()?;
    let mut lessons = snapshot
        .lessons
        .into_iter()
        .map(|lesson| (lesson.id.clone(), lesson))
        .collect::<BTreeMap<_, _>>();
    let mut new_ids = BTreeSet::new();
    for run in runs {
        for mutation in validate_run(session, run, directory).await? {
            if let Some(lesson) = stage(&store, &lessons, mutation)? {
                new_ids.insert(lesson.id.clone());
                lessons.insert(lesson.id.clone(), lesson);
            }
        }
    }
    snapshot.lessons = lessons.into_values().collect();
    files::bounded(&snapshot)?;
    Ok((snapshot, new_ids))
}

async fn validate_run(
    session: &Session,
    run: &HistoricalRun,
    directory: &Path,
) -> Result<Vec<LessonMutation>> {
    let loaded = history::discovery(session, run).await?;
    let context = loaded.context(session, run);
    let mut mutations = Vec::new();
    for task in &run.tasks {
        let (input, output) = tasks::response(directory, task)?;
        mutations.extend(
            context
                .mutations(task.role, &input, output)
                .map_err(|()| invalid(format!("invalid discovery result: {}", task.path)))?,
        );
    }
    Ok(mutations)
}

fn stage(
    store: &ActiveMemoryStore,
    lessons: &BTreeMap<String, Lesson>,
    mutation: LessonMutation,
) -> Result<Option<Lesson>> {
    let LessonMutation::Upsert { draft, .. } = mutation else {
        return Err(invalid("discovery attempted maintenance"));
    };
    // Existing live records win; retained tombstones must never be resurrected.
    if lessons.contains_key(&draft.id) || store.record(&draft.id)?.is_some() {
        return Ok(None);
    }
    Ok(Some(draft.into_lesson(1)))
}
