//! Enrich an already-failed required check with the failed job logs, without granting it merge
//! authority.
//!
//! The budgets, the redaction and the fallbacks are the maintained GitLab adapter's: the status
//! lines are kept, and a read that fails keeps them and adds why the details are unavailable. A run
//! for another revision is not this delivery's evidence and is skipped, so a diagnostic never
//! describes a run the delivery did not observe; a job that belongs to another run is a failure,
//! reported as the reason the details are unavailable rather than as the check outcome.

use std::collections::{BTreeMap, BTreeSet};

use crate::source_code_provider::adapters::core::ci_diagnostics;
use crate::source_code_provider::{SourceChecks, SourceDeliveryCredential, SourceDeliveryError};

use super::api::{Api, ApiRequest, PAGE_LIMIT, PAGE_SIZE};
use super::authority::split_repository;
use super::checks::{MAX_LINE_CHARS, printable};
use super::wire;

/// The repository and the head revision the failed check belongs to.
pub(super) struct FailureContext<'a> {
    pub repository: &'a str,
    pub head_revision: &'a str,
}

/// One failed job and the run it belongs to.
struct FailedJob {
    run: u64,
    id: u64,
    name: String,
}

pub(super) async fn include_failure_details(
    api: &Api,
    checks: &mut SourceChecks,
    context: &FailureContext<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    let SourceChecks::Failed { diagnostic } = checks else {
        // A check that did not fail has no job evidence to read.
        return Ok(());
    };
    let secrets = api.diagnostic_secrets(credential);
    // The status lines were redacted line by line when the check was observed; joining them again
    // would merge every failed check into one line.
    let status_lines = diagnostic.clone();

    let failed = match failed_jobs(api, context, credential).await {
        // A read that fails keeps the status lines and says why the details are missing, whether
        // the reason is a missing permission or an identity that changed while reading.
        Err(error) => {
            *diagnostic = format!("{status_lines}\nForgejo CI details unavailable: {error}");
            return Ok(());
        }
        Ok(jobs) => jobs,
    };

    let mut lines = vec![status_lines];
    for job in &failed {
        lines.push(format!(
            "- {} failed (run {}, job {})",
            ci_diagnostics::bounded_text(&printable(&secrets, &job.name), MAX_LINE_CHARS),
            job.run,
            job.id,
        ));
    }
    if failed.len() > ci_diagnostics::MAX_FAILED_CHECK_LOGS {
        lines.push(format!(
            "Log excerpts limited to {} of {} failed jobs; see the workflow runs",
            ci_diagnostics::MAX_FAILED_CHECK_LOGS,
            failed.len(),
        ));
    }
    let summary = ci_diagnostics::bounded_text(
        &lines.join("\n"),
        ci_diagnostics::MAX_FAILURE_DIAGNOSTIC_CHARS,
    );
    *diagnostic = summary;

    let (owner, name) = split_repository(context.repository)?;
    let mut logs = Vec::new();
    for job in failed.iter().take(ci_diagnostics::MAX_FAILED_CHECK_LOGS) {
        let id = job.id.to_string();
        let log = match api
            .job_log(
                &["repos", owner, name, "actions", "jobs", &id, "logs"],
                credential,
            )
            .await
        {
            Ok(log) => log,
            Err(error) => {
                ci_diagnostics::JobLog::unavailable(format!("Forgejo job log unavailable: {error}"))
            }
        };
        logs.push((job.id, log));
    }
    ci_diagnostics::include_job_logs(diagnostic, "Forgejo", &logs);
    Ok(())
}

/// The runs for the head revision. The run list carries its total in the body rather than in the
/// `x-total-count` header, so the page count is read from the response itself.
async fn failed_runs(
    api: &Api,
    context: &FailureContext<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Vec<wire::ActionRun>, SourceDeliveryError> {
    let (owner, name) = split_repository(context.repository)?;
    let mut collected: Vec<wire::ActionRun> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut reported = None;
    let mut complete = false;
    for page in 1..=PAGE_LIMIT {
        let list: wire::ActionRunList = api
            .json(
                ApiRequest::get(&["repos", owner, name, "actions", "runs"])
                    .query("head_sha", context.head_revision)
                    .query("limit", PAGE_SIZE.to_string())
                    .query("page", page.to_string()),
                credential,
            )
            .await?;
        let count = list.workflow_runs.len();
        if count > PAGE_SIZE {
            return Err(SourceDeliveryError::api(
                None,
                "Forgejo Actions page exceeded its item bound",
            ));
        }
        // The total the body reports bounds how much may be collected, so a page that repeats a run
        // cannot make the read look complete.
        let total = *reported.get_or_insert(list.total_count);
        for run in list.workflow_runs {
            if seen.insert(run.id) {
                collected.push(run);
            }
        }
        if collected.len() as u64 > total {
            return Err(SourceDeliveryError::api(
                None,
                format!(
                    "Forgejo Actions pagination returned more runs than its reported total of {total}"
                ),
            ));
        }
        if collected.len() as u64 == total {
            complete = true;
            break;
        }
        if count == 0 {
            // A shortened list could hide the run that failed, so an empty page is only the end of
            // the read when the reported total is already collected.
            return Err(SourceDeliveryError::api(
                None,
                format!(
                    "Forgejo Actions pagination returned an empty page before the reported total of {total}"
                ),
            ));
        }
    }
    if !complete {
        return Err(SourceDeliveryError::api(
            None,
            format!(
                "Forgejo Actions pagination exceeded its consecutive page bound of {PAGE_LIMIT}"
            ),
        ));
    }

    // Only the newest attempt of a workflow can be the one the check reported, and a run for
    // another revision is not this delivery's evidence: the complete read counted it, so the
    // reported total still covers it, but it is skipped here.
    let mut newest: BTreeMap<String, wire::ActionRun> = BTreeMap::new();
    for run in collected
        .into_iter()
        .filter(|run| run.commit_sha == context.head_revision)
    {
        let newest_attempt = newest
            .get(&run.workflow_id)
            .is_none_or(|kept| kept.id < run.id);
        if newest_attempt {
            newest.insert(run.workflow_id.clone(), run);
        }
    }
    let mut failed: Vec<wire::ActionRun> = newest
        .into_values()
        .filter(|run| run.status == "failure")
        .collect();
    failed.sort_by_key(|run| run.id);
    Ok(failed)
}

async fn failed_jobs(
    api: &Api,
    context: &FailureContext<'_>,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Vec<FailedJob>, SourceDeliveryError> {
    let runs = failed_runs(api, context, credential).await?;
    let (owner, name) = split_repository(context.repository)?;
    let mut failed = Vec::new();
    let mut seen = BTreeSet::new();
    for run in &runs {
        let run_id = run.id.to_string();
        let jobs: Vec<wire::ActionJob> = api
            .json(
                ApiRequest::get(&["repos", owner, name, "actions", "runs", &run_id, "jobs"]),
                credential,
            )
            .await?;
        for job in jobs {
            if job.run_id != run.id {
                return Err(SourceDeliveryError::identity(format!(
                    "Forgejo Actions job {} belongs to run {} instead of run {}",
                    job.id, job.run_id, run.id,
                )));
            }
            if job.status != "failure" || !seen.insert(job.id) {
                continue;
            }
            failed.push(FailedJob {
                run: run.id,
                id: job.id,
                name: job.name,
            });
        }
    }
    failed.sort_by_key(|job| job.id);
    Ok(failed)
}
