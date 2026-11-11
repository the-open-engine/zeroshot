//! Enrich an already-failed required pipeline without granting it merge authority.
use super::*;
use super::super::ci_diagnostics::{
    MAX_FAILED_CHECK_LOGS, MAX_FAILURE_DIAGNOSTIC_CHARS, MAX_FAILURE_ITEM_CHARS, bounded_text,
    include_job_logs, job_log_tail,
};
use wire::{Job, Pipeline};

pub(super) struct PipelineContext<'a> {
    pub head: Option<&'a Pipeline>,
    pub project: &'a Project,
}

pub(super) async fn include_failure_details(
    authority: &GitLabDeliveryAuthority,
    context: PipelineContext<'_>,
    observed: &mut SourceReviewObservation,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    let SourceReviewState::Open {
        checks: SourceChecks::Failed { diagnostic },
    } = &mut observed.state
    else {
        return Ok(());
    };
    let head = context.head.ok_or(SourceDeliveryError::Rejected)?;
    let id = head.id.to_string();
    let pipeline: Pipeline = authority
        .api
        .json(
            ApiRequest::get(&["projects", &observed.repository, "pipelines", &id]),
            credential,
        )
        .await?;
    require_pipeline(&pipeline, head)?;
    let mut jobs: Vec<Job> = authority
        .api
        .pages(
            &["projects", &observed.repository, "pipelines", &id, "jobs"],
            credential,
        )
        .await?;
    jobs.extend(
        authority
            .api
            .pages::<Job>(
                &[
                    "projects",
                    &observed.repository,
                    "pipelines",
                    &id,
                    "trigger_jobs",
                ],
                credential,
            )
            .await?,
    );
    let mut failures = vec![diagnostic.clone()];
    let errors = pipeline
        .yaml_errors
        .into_iter()
        .chain(super::pipeline_errors::read(authority, context.project, head, credential).await?);
    let mut seen_errors = BTreeSet::new();
    for error in errors {
        if !error.trim().is_empty() && seen_errors.insert(error.clone()) {
            failures.push(format!(
                "Pipeline configuration error: {}",
                failure_text(&error, credential)
            ));
        }
    }
    let mut failed_jobs = Vec::new();
    let mut seen = BTreeSet::new();
    for job in jobs.into_iter().filter(|job| job.status == "failed") {
        require_pipeline(&job.pipeline, head)?;
        if job.id == 0 {
            return Err(SourceDeliveryError::identity(
                "GitLab returned an invalid failed job ID",
            ));
        }
        if !seen.insert(job.id) {
            continue;
        }
        let item = format!(
            "{} (stage: {}) failed: {} (job {})",
            job.name,
            job.stage,
            job.failure_reason
                .as_deref()
                .unwrap_or("reason unavailable"),
            job.id,
        );
        failures.push(format!("- {}", failure_text(&item, credential)));
        failed_jobs.push(job.id);
    }
    if failed_jobs.len() > MAX_FAILED_CHECK_LOGS {
        failures.push(format!(
            "Log excerpts limited to {MAX_FAILED_CHECK_LOGS} of {} failed jobs; see pipeline jobs",
            failed_jobs.len(),
        ));
        failed_jobs.truncate(MAX_FAILED_CHECK_LOGS);
    }
    *diagnostic = bounded_text(&failures.join("\n"), MAX_FAILURE_DIAGNOSTIC_CHARS);
    let mut logs = Vec::new();
    for job in failed_jobs {
        let log = match authority
            .api
            .job_log(
                &[
                    "projects",
                    &observed.repository,
                    "jobs",
                    &job.to_string(),
                    "trace",
                ],
                credential,
            )
            .await
        {
            Ok(bytes) => {
                job_log_tail(redacted(&String::from_utf8_lossy(&bytes), credential).as_bytes())
            }
            Err(error) => format!("GitLab job log unavailable: {error}"),
        };
        logs.push((job, log));
    }
    include_job_logs(diagnostic, "GitLab", &logs);
    Ok(())
}

fn require_pipeline(value: &Pipeline, head: &Pipeline) -> Result<(), SourceDeliveryError> {
    if value.id != head.id
        || value.sha != head.sha
        || value
            .project_id
            .zip(head.project_id)
            .is_some_and(|(left, right)| left != right)
    {
        return Err(SourceDeliveryError::identity(
            "GitLab CI evidence changed pipeline identity",
        ));
    }
    Ok(())
}

fn failure_text(value: &str, credential: SourceDeliveryCredential<'_>) -> String {
    bounded_text(
        &one_line(&redacted(value, credential)),
        MAX_FAILURE_ITEM_CHARS,
    )
}

fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn redacted(value: &str, credential: SourceDeliveryCredential<'_>) -> String {
    crate::native_v2_delivery::command::redact(
        value,
        &[
            credential.expose().to_owned(),
            crate::native_v2_delivery::git_auth::encode_basic_credential(credential.expose()),
        ],
    )
}
