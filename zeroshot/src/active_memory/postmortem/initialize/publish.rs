use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Result, Review, Session, files, publication};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingPublication {
    id: String,
    review_digest: String,
    report: Value,
}

pub(super) async fn commit(session: &Session, directory: &Path) -> Result<Value> {
    let review: Review = files::read(&directory.join("review.json"))?;
    let digest = files::digest(&review)?;
    if let Some(report) = recover_matching(session, directory, Some(&digest))? {
        return Ok(report);
    }
    let publication = validate_publication(session, directory, &review).await?;
    files::write(&directory.join("publication.json"), &publication.pending)?;
    let versions = session.store()?.commit_initialization(
        &publication.pending.id,
        &publication.mutations,
        &publication.expected,
    )?;
    finish(directory, publication.pending, versions)
}

struct Publication {
    pending: PendingPublication,
    mutations: Vec<crate::active_memory::LessonMutation>,
    expected: Vec<(String, Option<u64>)>,
}

async fn validate_publication(
    session: &Session,
    directory: &Path,
    review: &Review,
) -> Result<Publication> {
    review.verify_source(session).await?;
    let (mutations, deferred) = publication::validate(session, review, directory).await?;
    review.verify_source(session).await?;
    let expected = review
        .snapshot
        .lessons
        .iter()
        .map(|lesson| {
            (
                lesson.id.clone(),
                (!review.new_ids.contains(&lesson.id)).then_some(lesson.revision),
            )
        })
        .collect();
    let pending = prepare_publication((session, directory), review, &mutations, deferred)?;
    Ok(Publication {
        pending,
        mutations,
        expected,
    })
}

pub(super) fn recover(session: &Session, directory: &Path) -> Result<Option<Value>> {
    recover_matching(session, directory, None)
}

fn recover_matching(
    session: &Session,
    directory: &Path,
    review_digest: Option<&str>,
) -> Result<Option<Value>> {
    let Some(pending) = files::optional::<PendingPublication>(&directory.join("publication.json"))?
    else {
        return Ok(None);
    };
    if review_digest.is_some_and(|expected| expected != pending.review_digest) {
        return Ok(None);
    }
    let Some(versions) = session.store()?.initialization_receipt(&pending.id)? else {
        return Ok(None);
    };
    finish(directory, pending, versions).map(Some)
}

fn finish(
    directory: &Path,
    mut pending: PendingPublication,
    versions: Vec<crate::active_memory::LessonVersion>,
) -> Result<Value> {
    pending.report["versions"] = serde_json::to_value(versions)?;
    files::write(&directory.join("committed.json"), &pending.report)?;
    Ok(pending.report)
}

pub(super) fn acknowledge(session: &Session, directory: &Path) {
    if let Ok(Some(pending)) =
        files::optional::<PendingPublication>(&directory.join("publication.json"))
    {
        // Complete is already durable in the private session. Failure only retains a bounded receipt.
        let _ = session
            .store()
            .and_then(|store| store.finish_initialization(&pending.id).map_err(Into::into));
    }
}

fn prepare_publication(
    location: (&Session, &Path),
    review: &Review,
    mutations: &[crate::active_memory::LessonMutation],
    deferred: usize,
) -> Result<PendingPublication> {
    let (session, directory) = location;
    let id = files::digest(&(&session.state_root, directory, review, mutations))?;
    let pending = PendingPublication {
        id,
        review_digest: files::digest(review)?,
        report: json!({"published_mutations":mutations.len(),
        "deferred":deferred,"reviewed_lessons":review.snapshot.lessons.len(),"source_digest":review.source_digest}),
    };
    Ok(pending)
}
