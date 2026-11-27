//! Failed-check CI evidence acceptance for the contributed Forgejo adapter. Every read goes
//! through the loopback fixture, so each test also shows which requests the observation made.
//! Each test names the acceptance criterion it verifies, from `D1` to `D13`.
use openengine_cluster_testkit::assertions::{AssertAt, AssertValue};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::source_code_provider::{
    adapters::core::ci_diagnostics, SourceChecks, SourceDeliveryCredential,
};

use super::api::{Api, PAGE_LIMIT, PAGE_SIZE, strip_log_timestamps};
use super::ci::{FailureContext, include_failure_details};
use super::http_fixture::{Fixture, Response};

const TOKEN: &str = "forgejo-test-token";
const REPOSITORY: &str = "owner/name";
const REVISION: &str = "0123456789012345678901234567890123456789";
const OTHER_REVISION: &str = "1111111111111111111111111111111111111111";
const STATUS_LINES: &str = "ci: failure: 3 tests failed";

fn credential() -> SourceDeliveryCredential<'static> {
    SourceDeliveryCredential::token(TOKEN)
}

fn context() -> FailureContext<'static> {
    FailureContext {
        repository: REPOSITORY,
        head_revision: REVISION,
    }
}

/// Reads the evidence for one failed check and returns the diagnostic it produced.
async fn enrich(fixture: &Fixture, status_lines: &str) -> String {
    let mut checks = SourceChecks::Failed {
        diagnostic: status_lines.to_owned(),
    };
    enrich_checks(fixture, &mut checks).await;
    let SourceChecks::Failed { diagnostic } = &checks else {
        panic!("a failed check stays failed");
    };
    diagnostic.clone()
}

async fn enrich_checks(fixture: &Fixture, checks: &mut SourceChecks) {
    let api = Api::loopback(&fixture.endpoint()).assert_value();
    include_failure_details(&api, checks, &context(), credential())
        .await
        .assert_value();
}

/// A run as the Actions list reports it, with the fields the wire shape ignores.
fn run(id: u64, commit_sha: &str, status: &str, workflow_id: &str) -> Value {
    json!({
        "id": id,
        "status": status,
        "commit_sha": commit_sha,
        "workflow_id": workflow_id,
        "event": "pull_request",
        "html_url": format!("https://forge.example/{REPOSITORY}/actions/runs/{id}"),
    })
}

/// The run list reports its total in the body, because this endpoint sends no `x-total-count`.
fn run_list(total: u64, runs: &[Value]) -> Response {
    Response::json(json!({
        "workflow_runs": runs,
        "total_count": total,
        "unknown": "the wire shape ignores unknown fields",
    }))
}

fn job(id: u64, run_id: u64, name: &str, status: &str) -> Value {
    json!({
        "id": id,
        "run_id": run_id,
        "attempt": 1,
        "name": name,
        "runs_on": ["docker"],
        "task_id": id,
        "status": status,
    })
}

fn job_list(jobs: &[Value]) -> Response {
    Response::json(json!(jobs))
}

/// A job log is plain text, not JSON.
fn log(body: &str) -> Response {
    Response::status(StatusCode::OK).with_body(body.to_owned())
}

fn runs_uri(page: u64) -> String {
    format!(
        "/api/v1/repos/{REPOSITORY}/actions/runs?head_sha={REVISION}&limit={PAGE_SIZE}&page={page}"
    )
}

fn jobs_uri(run: u64) -> String {
    format!("/api/v1/repos/{REPOSITORY}/actions/runs/{run}/jobs")
}

fn logs_uri(job: u64) -> String {
    format!("/api/v1/repos/{REPOSITORY}/actions/jobs/{job}/logs")
}

/// D1: a failed check keeps its status line and gains the job summary and the log finding, with
/// the timestamp prefix removed.
#[tokio::test]
async fn d1_a_failed_check_keeps_the_status_line_and_adds_the_log_finding_without_its_timestamp() {
    let fixture = Fixture::new().await;
    fixture
        .queue(run_list(1, &[run(1, REVISION, "failure", "ci.yml")]))
        .queue(job_list(&[job(1, 1, "test", "failure")]))
        .queue(log(
            "2026-11-20T16:00:16.6745055Z # duration_ms 42.0097\n\
             2026-11-20T16:00:16.8746225Z error: policy CL1: CHANGELOG.md does not mention the exported function `subtract` (src/rechner.js:5).\n\
             2026-11-20T16:00:16.9350583Z [runner]: exitcode '1': failure",
        ));

    let diagnostic = enrich(&fixture, STATUS_LINES).await;
    assert_eq!(
        diagnostic,
        "ci: failure: 3 tests failed\n\
         - test failed (run 1, job 1)\n\
         --- Forgejo job 1 log excerpt ---\n\
         # duration_ms 42.0097\n\
         error: policy CL1: CHANGELOG.md does not mention the exported function `subtract` (src/rechner.js:5).\n\
         [runner]: exitcode '1': failure"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert_eq!(requests.assert_at(0).uri, runs_uri(1));
    assert_eq!(requests.assert_at(1).uri, jobs_uri(1));
    assert_eq!(requests.assert_at(2).uri, logs_uri(1));
}

/// D13: several failed statuses keep one line each, so every failed check stays readable.
#[tokio::test]
async fn d13_several_status_lines_keep_their_line_breaks() {
    let fixture = Fixture::new().await;
    fixture
        .queue(run_list(1, &[run(1, REVISION, "failure", "ci.yml")]))
        .queue(job_list(&[job(1, 1, "test", "failure")]))
        .queue(log("error: failed"));

    let diagnostic = enrich(
        &fixture,
        "build: failure
test: failure",
    )
    .await;
    assert!(
        diagnostic.starts_with(
            "build: failure
test: failure
- test failed (run 1, job 1)"
        ),
        "{diagnostic}"
    );
}

/// D2: a check that did not fail has no job evidence, so nothing is read for it.
#[tokio::test]
async fn d2_checks_that_are_not_failed_read_no_runs() {
    for state in [
        SourceChecks::Passed,
        SourceChecks::Pending,
        SourceChecks::NotRequired,
    ] {
        let fixture = Fixture::new().await;
        let mut checks = state.clone();
        enrich_checks(&fixture, &mut checks).await;
        assert_eq!(checks, state, "the check keeps its own outcome");
        assert!(
            fixture.requests().is_empty(),
            "nothing is read for {state:?}"
        );
    }
}

/// D3: only the newest attempt of a workflow can be the run the check reported.
#[tokio::test]
async fn d3_only_the_newest_run_of_a_workflow_reports_its_failed_jobs() {
    let fixture = Fixture::new().await;
    fixture
        .queue(run_list(
            2,
            &[
                run(1, REVISION, "failure", "ci.yml"),
                run(5, REVISION, "failure", "ci.yml"),
            ],
        ))
        .queue(job_list(&[job(9, 5, "test", "failure")]))
        .queue(log("error: the newest attempt is the one that failed"));

    let diagnostic = enrich(&fixture, STATUS_LINES).await;
    assert_eq!(
        diagnostic,
        "ci: failure: 3 tests failed\n\
         - test failed (run 5, job 9)\n\
         --- Forgejo job 9 log excerpt ---\n\
         error: the newest attempt is the one that failed"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert_eq!(requests.assert_at(1).uri, jobs_uri(5));
    assert_eq!(requests.assert_at(2).uri, logs_uri(9));
}

/// D4: a run for another revision is not this delivery's evidence, so it is skipped, and the
/// reported total still covers it.
#[tokio::test]
async fn d4_a_run_for_another_commit_revision_is_skipped_after_the_complete_read() {
    let fixture = Fixture::new().await;
    fixture.queue(run_list(1, &[run(1, OTHER_REVISION, "failure", "ci.yml")]));

    let diagnostic = enrich(&fixture, STATUS_LINES).await;
    assert_eq!(
        diagnostic, STATUS_LINES,
        "no job evidence is read for a run that is not this delivery's"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests.assert_at(0).uri, runs_uri(1));
}

/// D5: a job that belongs to another run cannot be described as this run's failure, so the status
/// lines stay and the reason is reported as unavailable details.
#[tokio::test]
async fn d5_a_job_of_another_run_keeps_the_status_lines_and_names_the_reason() {
    let fixture = Fixture::new().await;
    fixture
        .queue(run_list(1, &[run(1, REVISION, "failure", "ci.yml")]))
        .queue(job_list(&[job(3, 2, "test", "failure")]));

    let diagnostic = enrich(&fixture, STATUS_LINES).await;
    assert!(diagnostic.starts_with(STATUS_LINES), "{diagnostic}");
    assert!(
        diagnostic.contains(
            "Forgejo CI details unavailable: source delivery identity mismatch: Forgejo Actions \
             job 3 belongs to run 2 instead of run 1"
        ),
        "{diagnostic}"
    );
}

/// D6: the pages are joined by the total the body reports, and a page that stops early cannot
/// stand for the whole list.
#[tokio::test]
async fn d6_two_pages_are_joined_by_the_reported_total_and_an_early_stop_is_an_api_error() {
    let joined = Fixture::new().await;
    joined
        .queue(run_list(2, &[run(1, REVISION, "success", "ci.yml")]))
        .queue(run_list(2, &[run(2, REVISION, "success", "ci.yml")]));
    assert_eq!(enrich(&joined, STATUS_LINES).await, STATUS_LINES);

    let requests = joined.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert_eq!(requests.assert_at(0).uri, runs_uri(1));
    assert_eq!(requests.assert_at(1).uri, runs_uri(2));

    // An empty page before the reported total could hide the run that failed, so it is an API
    // error, and a non-identity error keeps the status lines with the reason appended.
    let shortened = Fixture::new().await;
    shortened
        .queue(run_list(2, &[run(1, REVISION, "success", "ci.yml")]))
        .queue(run_list(2, &[]));
    let diagnostic = enrich(&shortened, STATUS_LINES).await;
    assert!(diagnostic.starts_with(STATUS_LINES), "{diagnostic}");
    assert!(
        diagnostic.contains(
            "Forgejo CI details unavailable: source API request failed: Forgejo Actions \
             pagination returned an empty page before the reported total of 2"
        ),
        "{diagnostic}"
    );
    let requests = shortened.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");

    // A page larger than the bound is an API error too, and the shared page bound stays the same.
    let oversized = Fixture::new().await;
    let runs = (1..=PAGE_SIZE as u64 + 1)
        .map(|id| run(id, REVISION, "success", "ci.yml"))
        .collect::<Vec<Value>>();
    oversized.queue(run_list(runs.len() as u64, &runs));

    let diagnostic = enrich(&oversized, STATUS_LINES).await;
    assert!(diagnostic.starts_with(STATUS_LINES), "{diagnostic}");
    assert!(
        diagnostic.contains(
            "Forgejo CI details unavailable: source API request failed: Forgejo Actions page \
             exceeded its item bound"
        ),
        "{diagnostic}"
    );
    assert_eq!(PAGE_LIMIT, 64, "the consecutive page bound stays shared");
}

/// D7: a log that cannot be read says so, and the failed check itself is still reported.
#[tokio::test]
async fn d7_an_unreadable_log_keeps_the_failed_check_and_names_why_it_is_unavailable() {
    let fixture = Fixture::new().await;
    fixture
        .queue(run_list(1, &[run(1, REVISION, "failure", "ci.yml")]))
        .queue(job_list(&[job(1, 1, "test", "failure")]))
        .queue(Response::status(StatusCode::INTERNAL_SERVER_ERROR));

    let diagnostic = enrich(&fixture, STATUS_LINES).await;
    assert!(diagnostic.starts_with(STATUS_LINES), "{diagnostic}");
    assert!(
        diagnostic.contains("- test failed (run 1, job 1)"),
        "{diagnostic}"
    );
    assert!(
        diagnostic
            .contains("Forgejo job log unavailable: source API request failed: Forgejo API GET"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("/actions/jobs/1/logs returned HTTP 500"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains(TOKEN), "{diagnostic}");

    let requests = fixture.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    assert_eq!(requests.assert_at(2).uri, logs_uri(1));
}

/// D8: a runs read that is refused keeps the status lines, so the failed check is not hidden.
#[tokio::test]
async fn d8_a_refused_runs_read_keeps_the_status_lines_and_reports_why_details_are_missing() {
    let fixture = Fixture::new().await;
    fixture.queue(Response::status(StatusCode::FORBIDDEN));

    let diagnostic = enrich(&fixture, STATUS_LINES).await;
    assert!(diagnostic.starts_with(STATUS_LINES), "{diagnostic}");
    assert!(
        diagnostic
            .contains("Forgejo CI details unavailable: source API request failed: Forgejo API GET"),
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("/actions/runs returned HTTP 403"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains(TOKEN), "{diagnostic}");

    let requests = fixture.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests.assert_at(0).uri, runs_uri(1));
}

/// D9: the shared log budget caps the excerpts, and the diagnostic says how many jobs it left out.
#[tokio::test]
async fn d9_nine_failed_jobs_read_exactly_eight_logs_and_report_the_limit() {
    let fixture = Fixture::new().await;
    let jobs = (1..=9)
        .map(|id| job(id, 1, &format!("job {id}"), "failure"))
        .collect::<Vec<Value>>();
    fixture.queue(run_list(1, &[run(1, REVISION, "failure", "ci.yml")]));
    fixture.queue(job_list(&jobs));
    for id in 1..=8 {
        fixture.queue(log(&format!("error: job {id} failed")));
    }

    let diagnostic = enrich(&fixture, STATUS_LINES).await;
    assert!(
        diagnostic.contains("Log excerpts limited to 8 of 9 failed jobs; see the workflow runs"),
        "{diagnostic}"
    );
    assert_eq!(
        diagnostic.matches("--- Forgejo job ").count(),
        ci_diagnostics::MAX_FAILED_CHECK_LOGS,
        "{diagnostic}"
    );
    assert!(
        diagnostic.contains("- job 9 failed (run 1, job 9)"),
        "{diagnostic}"
    );

    let requests = fixture.requests();
    assert_eq!(requests.len(), 10, "{requests:?}");
    for (index, id) in (1..=8).enumerate() {
        assert_eq!(
            requests.assert_at(index + 2).uri,
            logs_uri(id),
            "{requests:?}"
        );
    }
}

/// D10: a log that echoes the credential is redacted by the same shared secret set as every other
/// diagnostic.
#[tokio::test]
async fn d10_a_log_echoing_the_token_never_shows_it() {
    let fixture = Fixture::new().await;
    fixture
        .queue(run_list(1, &[run(1, REVISION, "failure", "ci.yml")]))
        .queue(job_list(&[job(1, 1, "test", "failure")]))
        .queue(log(&format!("error: runner accepted token {TOKEN}")));

    let api = Api::loopback(&fixture.endpoint()).assert_value();
    let secrets = api.diagnostic_secrets(credential());
    assert!(secrets.iter().any(|secret| secret == TOKEN), "{secrets:?}");

    let diagnostic = enrich(&fixture, STATUS_LINES).await;
    assert!(!diagnostic.contains(TOKEN), "{diagnostic}");
    assert!(diagnostic.contains("[REDACTED]"), "{diagnostic}");
}

/// D11: an oversized log keeps its failure line, as the shared tail bound guarantees.
#[tokio::test]
async fn d11_an_oversized_log_keeps_the_failure_line_within_the_shared_tail_bound() {
    let fixture = Fixture::new().await;
    let mut body = String::from("start-of-the-log-marker\n");
    for _ in 0..2_000 {
        body.push_str("filler line that is long enough to make the log exceed its tail bound\n");
    }
    body.push_str("error: the failure line is kept at the end");

    fixture
        .queue(run_list(1, &[run(1, REVISION, "failure", "ci.yml")]))
        .queue(job_list(&[job(1, 1, "test", "failure")]))
        .queue(log(&body));

    let diagnostic = enrich(&fixture, STATUS_LINES).await;
    assert!(
        diagnostic.contains("error: the failure line is kept at the end"),
        "{diagnostic}"
    );
    assert!(
        !diagnostic.contains("start-of-the-log-marker"),
        "{diagnostic}"
    );
    // `MAX_JOB_LOG_TAIL_BYTES` stays private to the shared module, so the bound is the value the
    // GitLab and Forgejo reads share.
    assert!(
        diagnostic.chars().count() <= 64 * 1024,
        "the excerpt stays inside the shared tail bound: {diagnostic}"
    );
}

/// D12: only a prefix at the start of a line is a timestamp, so a line that merely looks like one
/// in the middle keeps its text.
#[tokio::test]
async fn d12_only_a_line_starting_with_a_stamp_loses_its_prefix() {
    assert_eq!(
        strip_log_timestamps("2026-11-20T16:00:16.6745055Z # duration_ms 42.0097"),
        "# duration_ms 42.0097"
    );
    assert_eq!(
        strip_log_timestamps("error: policy CL1: CHANGELOG.md is missing"),
        "error: policy CL1: CHANGELOG.md is missing"
    );
    assert_eq!(
        strip_log_timestamps("output 2026-11-20T16:00:16.6745055Z in the middle of the line"),
        "output 2026-11-20T16:00:16.6745055Z in the middle of the line"
    );
    assert_eq!(
        strip_log_timestamps(
            "2026-11-20T16:00:16.6745055Z first line\n\
             unstamped second line\n\
             2026-11-20T16:00:16.8746225Z third line"
        ),
        "first line\nunstamped second line\nthird line"
    );
    // A stamp must end at `Z` and be followed by exactly one space.
    assert_eq!(
        strip_log_timestamps("2026-11-20T16:00:16.6745055 no trailing Z"),
        "2026-11-20T16:00:16.6745055 no trailing Z"
    );
    assert_eq!(
        strip_log_timestamps("2026-11-20T16:00:16.6745055Z  two spaces"),
        " two spaces",
        "the bound is exactly one space, so the second one remains"
    );
}
