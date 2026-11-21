//! What an open pull request is observed to be waiting for.
//!
//! Three reads answer this: the pull request itself, because Forgejo reports `mergeable` as false
//! while its own conflict check is still running; the rule in effect for the target branch, which
//! the branch endpoint reports without administrator rights; and the statuses of the head revision.
//! Approvals are read only when the rule requires them. `branch_protections` is never read, and no
//! workflow run, job or log is read here.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::native_v2_delivery::command;
use crate::source_code_provider::{
    SourceChecks, SourceDeliveryCredential, SourceDeliveryError, SourceReviewState,
};

use super::api::ApiRequest;
use super::authority::{ForgejoDeliveryAuthority, split_repository};
use super::wire;

pub(super) const MAX_LINE_CHARS: usize = 1024;
pub(super) const MAX_DIAGNOSTIC_CHARS: usize = 8192;

/// The most Forgejo will be read for one ambiguous `mergeable`, and the wait before each read.
pub(super) const MERGEABILITY_READS: usize = 3;
pub(super) const MERGEABILITY_DELAY: Duration = Duration::from_secs(2);

/// The observation of an open pull request, and whether it is ready and waiting for a person.
pub(super) struct OpenReview {
    pub state: SourceReviewState,
    pub pull_request_ready: bool,
}

impl OpenReview {
    pub(super) fn new(state: SourceReviewState, pull_request_ready: bool) -> Self {
        Self {
            state,
            pull_request_ready,
        }
    }
}

/// The rule in effect for a branch, as the branch endpoint reports it.
struct Rule {
    enforced: bool,
    contexts: Vec<String>,
    required_approvals: u64,
}

#[derive(PartialEq, Eq)]
enum StatusMeaning {
    Passed,
    Pending,
    Failed,
}

pub(super) async fn observe_open(
    authority: &ForgejoDeliveryAuthority,
    repository: &str,
    pull: &wire::PullRequest,
    credential: SourceDeliveryCredential<'_>,
) -> Result<OpenReview, SourceDeliveryError> {
    if !pull.mergeable {
        let Some(rechecked) = recheck_mergeability(authority, repository, pull, credential).await?
        else {
            // Still ambiguous after every read: this is what the conflict state means here.
            return Ok(OpenReview::new(SourceReviewState::Conflict, false));
        };
        return observe_with_rule(authority, repository, &rechecked, credential).await;
    }
    observe_with_rule(authority, repository, pull, credential).await
}

async fn observe_with_rule(
    authority: &ForgejoDeliveryAuthority,
    repository: &str,
    pull: &wire::PullRequest,
    credential: SourceDeliveryCredential<'_>,
) -> Result<OpenReview, SourceDeliveryError> {
    // A draft is never ready, whatever its checks say, and nothing further is read for it.
    if pull.draft {
        return Ok(OpenReview::new(
            SourceReviewState::Open {
                checks: SourceChecks::Pending,
            },
            false,
        ));
    }
    let rule = rule_in_effect(authority, repository, &pull.base, credential).await?;
    let statuses = read_statuses(authority, repository, &pull.head.sha, credential).await?;
    let counted = counted_statuses(&statuses, &rule);
    let checks = checks_state(
        &authority.api.diagnostic_secrets(credential),
        &rule,
        &counted,
    )?;
    let required = usize::try_from(rule.required_approvals).unwrap_or(usize::MAX);
    let ready = matches!(checks, SourceChecks::Passed | SourceChecks::NotRequired)
        && required > 0
        && approved_reviews(authority, repository, pull.number, credential)
            .await?
            .iter()
            .filter(|review| counts_as_approval(review))
            .count()
            < required;
    Ok(OpenReview::new(SourceReviewState::Open { checks }, ready))
}

/// Reads the pull request again while `mergeable` is false. `None` means it stayed false through
/// every read; a read that disagrees with the pull it is rechecking is a temporary API error, so
/// delivery observes again rather than deciding from two answers that do not describe one pull.
async fn recheck_mergeability(
    authority: &ForgejoDeliveryAuthority,
    repository: &str,
    pull: &wire::PullRequest,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Option<wire::PullRequest>, SourceDeliveryError> {
    let (owner, name) = split_repository(repository)?;
    let number = pull.number.to_string();
    for _ in 0..MERGEABILITY_READS {
        tokio::time::sleep(MERGEABILITY_DELAY).await;
        let observed = authority
            .api
            .json::<wire::PullRequest>(
                ApiRequest::get(&["repos", owner, name, "pulls", number.as_str()]),
                credential,
            )
            .await?;
        if observed.state != "open" {
            return Err(SourceDeliveryError::api(
                None,
                format!(
                    "Forgejo pull request {} reported {} as its state while its mergeability was rechecked",
                    pull.number,
                    wire::observed(&observed.state),
                ),
            )
            .temporary());
        }
        if observed.head.sha != pull.head.sha {
            return Err(SourceDeliveryError::api(
                None,
                format!(
                    "Forgejo pull request {} returned {} as its head revision while its mergeability was rechecked, instead of {}",
                    pull.number,
                    wire::revision_identity(&observed.head.sha),
                    wire::revision_identity(&pull.head.sha),
                ),
            )
            .temporary());
        }
        if observed.mergeable {
            return Ok(Some(observed));
        }
    }
    Ok(None)
}

async fn rule_in_effect(
    authority: &ForgejoDeliveryAuthority,
    repository: &str,
    branch: &wire::PullBranch,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Rule, SourceDeliveryError> {
    let (owner, name) = split_repository(repository)?;
    let mut path = vec!["repos", owner, name, "branches"];
    path.extend(branch.reference.split('/'));
    let observed = match authority
        .api
        .json::<wire::Branch>(ApiRequest::get(&path), credential)
        .await
    {
        Ok(observed) => observed,
        Err(error) if error.api_status() == Some(404) => {
            return Err(SourceDeliveryError::identity(format!(
                "No Forgejo rule is in effect for branch {}, because the branch read returned no branch",
                wire::branch_identity(&branch.reference),
            )));
        }
        Err(error) => return Err(error),
    };
    Ok(Rule {
        enforced: observed.enable_status_check,
        contexts: observed.status_check_contexts,
        required_approvals: observed.required_approvals,
    })
}

/// The status history of a revision, keeping the newest entry for each context: an older failure
/// followed by a newer success is a pass, and the reverse is a failure.
async fn read_statuses(
    authority: &ForgejoDeliveryAuthority,
    repository: &str,
    revision: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Vec<wire::CommitStatus>, SourceDeliveryError> {
    let (owner, name) = split_repository(repository)?;
    let values = authority
        .api
        .pages::<wire::CommitStatus>(
            ApiRequest::get(&["repos", owner, name, "commits", revision, "statuses"]),
            credential,
        )
        .await?;
    let mut newest: BTreeMap<String, wire::CommitStatus> = BTreeMap::new();
    for status in values {
        status_meaning(&status)?;
        let keep = match newest.get(&status.context) {
            Some(kept) => kept.id < status.id,
            None => true,
        };
        if keep {
            newest.insert(status.context.clone(), status);
        }
    }
    Ok(newest.into_values().collect())
}

fn status_meaning(status: &wire::CommitStatus) -> Result<StatusMeaning, SourceDeliveryError> {
    match status.status.as_str() {
        "success" | "warning" | "skipped" => Ok(StatusMeaning::Passed),
        "pending" => Ok(StatusMeaning::Pending),
        "failure" | "error" => Ok(StatusMeaning::Failed),
        other => Err(SourceDeliveryError::api(
            None,
            format!(
                "Forgejo status {} reported {} as its result",
                wire::observed(&status.context),
                wire::observed(other),
            ),
        )),
    }
}

/// Which statuses the rule counts: the required contexts when they are named, and every status
/// otherwise. The order is the context order the diagnostic reports.
fn counted_statuses<'a>(
    statuses: &'a [wire::CommitStatus],
    rule: &Rule,
) -> Vec<&'a wire::CommitStatus> {
    if rule.enforced && !rule.contexts.is_empty() {
        return statuses
            .iter()
            .filter(|status| {
                rule.contexts
                    .iter()
                    .any(|pattern| context_matches(pattern, &status.context))
            })
            .collect();
    }
    statuses.iter().collect()
}

fn checks_state(
    secrets: &[String],
    rule: &Rule,
    counted: &[&wire::CommitStatus],
) -> Result<SourceChecks, SourceDeliveryError> {
    let mut failed = Vec::new();
    let mut pending = false;
    for status in counted {
        match status_meaning(status)? {
            StatusMeaning::Failed => failed.push(*status),
            StatusMeaning::Pending => pending = true,
            StatusMeaning::Passed => {}
        }
    }
    if !failed.is_empty() {
        return Ok(SourceChecks::Failed {
            diagnostic: diagnostic(secrets, &failed),
        });
    }
    if pending {
        return Ok(SourceChecks::Pending);
    }
    let unreported = rule.contexts.iter().any(|pattern| {
        !counted
            .iter()
            .any(|status| context_matches(pattern, &status.context))
    });
    if rule.enforced && (counted.is_empty() || unreported) {
        // A named context with no status, or an enforced check with no status at all, keeps the
        // delivery waiting rather than passing by absence.
        return Ok(SourceChecks::Pending);
    }
    if !counted.is_empty() {
        return Ok(SourceChecks::Passed);
    }
    Ok(SourceChecks::NotRequired)
}

async fn approved_reviews(
    authority: &ForgejoDeliveryAuthority,
    repository: &str,
    number: u64,
    credential: SourceDeliveryCredential<'_>,
) -> Result<Vec<wire::PullReview>, SourceDeliveryError> {
    let (owner, name) = split_repository(repository)?;
    let number = number.to_string();
    authority
        .api
        .pages::<wire::PullReview>(
            ApiRequest::get(&["repos", owner, name, "pulls", number.as_str(), "reviews"]),
            credential,
        )
        .await
}

fn counts_as_approval(review: &wire::PullReview) -> bool {
    review.state == "APPROVED" && review.official && !review.stale && !review.dismissed
}

/// A pattern matches by equality first. Otherwise `*` is any run and `?` is exactly one character;
/// a pattern with a bracket or a brace is not a glob Forgejo can match, so it keeps the delivery
/// waiting instead of passing by accident.
pub(super) fn context_matches(pattern: &str, context: &str) -> bool {
    if pattern == context {
        return true;
    }
    if pattern
        .chars()
        .any(|character| matches!(character, '[' | ']' | '{' | '}' | '\\'))
    {
        return false;
    }
    let pattern = pattern.chars().collect::<Vec<char>>();
    let context = context.chars().collect::<Vec<char>>();
    matches_glob(&pattern, &context)
}

fn matches_glob(pattern: &[char], context: &[char]) -> bool {
    let mut pattern_index = 0;
    let mut context_index = 0;
    let mut star: Option<(usize, usize)> = None;
    while context_index < context.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == '*' {
            star = Some((pattern_index, context_index));
            pattern_index += 1;
            continue;
        }
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == '?' || pattern[pattern_index] == context[context_index])
        {
            pattern_index += 1;
            context_index += 1;
            continue;
        }
        let Some((star_pattern, star_context)) = star else {
            return false;
        };
        // That `*` can still stand for a longer run, so the same run is tried again.
        let next = star_context + 1;
        star = Some((star_pattern, next));
        pattern_index = star_pattern + 1;
        context_index = next;
    }
    pattern[pattern_index..]
        .iter()
        .all(|character| *character == '*')
}

/// One line per failed status, in context order, with the token redacted the way `api.rs` redacts
/// response text and control characters turned into spaces.
fn diagnostic(secrets: &[String], failed: &[&wire::CommitStatus]) -> String {
    let lines = failed
        .iter()
        .map(|status| {
            let line = if status.description.is_empty() {
                format!(
                    "{}: {}",
                    printable(secrets, &status.context),
                    printable(secrets, &status.status),
                )
            } else {
                format!(
                    "{}: {}: {}",
                    printable(secrets, &status.context),
                    printable(secrets, &status.status),
                    printable(secrets, &status.description),
                )
            };
            bound(&line, MAX_LINE_CHARS)
        })
        .collect::<Vec<String>>();
    bound(&lines.join("\n"), MAX_DIAGNOSTIC_CHARS)
}

pub(super) fn printable(secrets: &[String], part: &str) -> String {
    command::redact(part, secrets).replace(char::is_control, " ")
}

fn bound(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}
