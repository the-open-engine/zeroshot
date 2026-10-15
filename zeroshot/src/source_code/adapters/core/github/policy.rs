use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value;

use super::*;

#[path = "policy/checks.rs"]
mod checks;
use checks::{RequiredChecks, classify_required_checks, failure_diagnostic};

const POLICY_QUERY: &str = r#"
query($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    nameWithOwner
    pullRequest(number: $number) {
      id
      number
      state
      merged
      mergeCommit { oid }
      mergeable
      mergeStateStatus
      reviewDecision
      isDraft
      isInMergeQueue
      isMergeQueueEnabled
      baseRefName
      baseRef {
        name
        refUpdateRule {
          requiredApprovingReviewCount
          requiredStatusCheckContexts
          requiresCodeOwnerReviews
          requiresConversationResolution
          requiresLinearHistory
          requiresSignatures
        }
      }
      headRefName
      headRefOid
      commits(last: 1) {
        nodes {
          commit {
            statusCheckRollup {
              contexts(first: 100, after: $endCursor) {
                pageInfo { hasNextPage endCursor }
                nodes {
                  __typename
                  ... on CheckRun {
                    name
                    status
                    conclusion
                    detailsUrl
                    databaseId
                    isRequired(pullRequestNumber: $number)
                  }
                  ... on StatusContext {
                    context
                    state
                    description
                    targetUrl
                    isRequired(pullRequestNumber: $number)
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#;

const MAX_FAILURE_DIAGNOSTIC_CHARS: usize = 8 * 1_024;
const MAX_FAILURE_FEEDBACK_CHARS: usize = 64 * 1_024;

pub(super) struct PolicySnapshot {
    pub(super) state: SourceReviewState,
    pub(super) failed_job_ids: Vec<u64>,
    pub(super) is_merge_queue_enabled: bool,
    pub(super) head_update: Option<HeadUpdate>,
    pub(super) pull_request_ready: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HeadUpdate(pub(super) String);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct PullRequestPolicyWire {
    id: String,
    number: u64,
    state: String,
    merged: bool,
    merge_commit: Option<RevisionWire>,
    mergeable: String,
    merge_state_status: String,
    review_decision: Option<String>,
    is_draft: bool,
    is_in_merge_queue: bool,
    is_merge_queue_enabled: bool,
    base_ref_name: String,
    base_ref: RefWire,
    head_ref_name: String,
    head_ref_oid: String,
    commits: CommitConnectionWire,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct RefWire {
    name: String,
    ref_update_rule: Option<RefUpdateRuleWire>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct RefUpdateRuleWire {
    required_approving_review_count: u64,
    required_status_check_contexts: Option<Vec<Option<String>>>,
    requires_code_owner_reviews: bool,
    requires_conversation_resolution: bool,
    requires_linear_history: bool,
    requires_signatures: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct RevisionWire {
    oid: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct CommitConnectionWire {
    nodes: Vec<CommitNodeWire>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct CommitNodeWire {
    commit: CommitWire,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct CommitWire {
    status_check_rollup: Option<StatusCheckRollupWire>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
struct StatusCheckRollupWire {
    contexts: CheckContextConnectionWire,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct CheckContextConnectionWire {
    page_info: PageInfoWire,
    nodes: Vec<CheckContextWire>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct PageInfoWire {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "__typename")]
enum CheckContextWire {
    CheckRun {
        name: String,
        status: String,
        conclusion: Option<String>,
        #[serde(rename = "detailsUrl")]
        details_url: Option<String>,
        #[serde(rename = "databaseId")]
        database_id: Option<u64>,
        #[serde(rename = "isRequired")]
        is_required: bool,
    },
    StatusContext {
        context: String,
        state: String,
        description: Option<String>,
        #[serde(rename = "targetUrl")]
        target_url: Option<String>,
        #[serde(rename = "isRequired")]
        is_required: bool,
    },
}

#[derive(Deserialize)]
struct QueryPageWire {
    data: QueryDataWire,
}

#[derive(Deserialize)]
struct QueryDataWire {
    repository: Option<RepositoryPolicyWire>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryPolicyWire {
    name_with_owner: String,
    pull_request: Option<PullRequestPolicyWire>,
}

pub(super) fn query_arguments(
    review: &SourceReviewReceipt,
) -> Result<Vec<String>, SourceDeliveryError> {
    review_query_arguments(review, POLICY_QUERY)
}

pub(super) fn classify_policy(
    value: Value,
    review: &SourceReviewReceipt,
) -> Result<PolicySnapshot, SourceDeliveryError> {
    let pages: Vec<QueryPageWire> = serde_json::from_value(value).map_err(|error| {
        SourceDeliveryError::api(
            None,
            format!("GitHub returned invalid delivery policy: {error}"),
        )
    })?;
    let first = pages.first().ok_or(SourceDeliveryError::Rejected)?;
    let first_repository = first
        .data
        .repository
        .as_ref()
        .ok_or(SourceDeliveryError::Rejected)?;
    let first_pull_request = require_identity(first_repository, review)?.clone();
    let (contexts, complete) = collect_contexts(&pages, review, first_repository)?;
    if !complete {
        return Ok(waiting_snapshot());
    }
    if let Some(snapshot) = terminal_snapshot(&first_pull_request)? {
        return Ok(snapshot);
    }
    Ok(classify_snapshot(&first_pull_request, &contexts))
}

fn collect_contexts(
    pages: &[QueryPageWire],
    review: &SourceReviewReceipt,
    first_repository: &RepositoryPolicyWire,
) -> Result<(Vec<CheckContextWire>, bool), SourceDeliveryError> {
    let mut contexts = Vec::new();
    let mut complete = true;
    let mut cursors = BTreeSet::new();

    for (index, page) in pages.iter().enumerate() {
        let repository = page
            .data
            .repository
            .as_ref()
            .ok_or(SourceDeliveryError::Rejected)?;
        let pull_request = require_identity(repository, review)?;
        complete &= page_policy_is_stable(first_repository, pull_request)?;
        let Some(page_contexts) = check_contexts(pull_request)? else {
            complete &= pages.len() == 1;
            continue;
        };
        complete &= page_is_complete(page_contexts, index, pages.len(), &mut cursors);
        contexts.extend(page_contexts.nodes.iter().cloned());
    }
    Ok((contexts, complete))
}

fn page_policy_is_stable(
    first_repository: &RepositoryPolicyWire,
    pull_request: &PullRequestPolicyWire,
) -> Result<bool, SourceDeliveryError> {
    let first_pull_request = first_repository
        .pull_request
        .as_ref()
        .ok_or(SourceDeliveryError::Rejected)?;
    Ok(same_policy(first_pull_request, pull_request))
}

fn page_is_complete(
    contexts: &CheckContextConnectionWire,
    index: usize,
    page_count: usize,
    cursors: &mut BTreeSet<String>,
) -> bool {
    let pagination_matches = contexts.page_info.has_next_page != (index + 1 == page_count);
    match contexts.page_info.end_cursor.as_ref() {
        Some(cursor) => pagination_matches && cursors.insert(cursor.clone()),
        None => pagination_matches && !contexts.page_info.has_next_page,
    }
}

pub(super) fn include_check_logs(snapshot: &mut PolicySnapshot, logs: &[(u64, String)]) {
    let SourceReviewState::Open {
        checks: SourceChecks::Failed { diagnostic },
    } = &mut snapshot.state
    else {
        return;
    };
    if logs.is_empty() {
        return;
    }
    let budget = MAX_FAILURE_FEEDBACK_CHARS.saturating_sub(diagnostic.chars().count()) / logs.len();
    for (job, log) in logs {
        let heading = format!("\n--- GitHub Actions job {job} log excerpt ---\n");
        let excerpt = log_tail(log, budget.saturating_sub(heading.chars().count()));
        diagnostic.push_str(&heading);
        diagnostic.push_str(&excerpt);
    }
}

fn require_identity<'a>(
    repository: &'a RepositoryPolicyWire,
    review: &SourceReviewReceipt,
) -> Result<&'a PullRequestPolicyWire, SourceDeliveryError> {
    let pull_request = repository
        .pull_request
        .as_ref()
        .ok_or(SourceDeliveryError::Rejected)?;
    let valid = repository.name_with_owner == review.repository
        && pull_request.number.to_string() == review.review_id
        && pull_request.base_ref_name == review.target_branch
        && pull_request.base_ref.name == review.target_branch
        && pull_request.head_ref_name == review.head_branch
        && pull_request.head_ref_oid == review.head_revision;
    valid
        .then_some(pull_request)
        .ok_or(SourceDeliveryError::Rejected)
}

fn same_policy(left: &PullRequestPolicyWire, right: &PullRequestPolicyWire) -> bool {
    (
        &left.id,
        left.number,
        &left.state,
        left.merged,
        &left.merge_commit,
        &left.mergeable,
        &left.merge_state_status,
        &left.review_decision,
        left.is_draft,
        left.is_in_merge_queue,
        left.is_merge_queue_enabled,
        &left.base_ref,
    ) == (
        &right.id,
        right.number,
        &right.state,
        right.merged,
        &right.merge_commit,
        &right.mergeable,
        &right.merge_state_status,
        &right.review_decision,
        right.is_draft,
        right.is_in_merge_queue,
        right.is_merge_queue_enabled,
        &right.base_ref,
    )
}

fn check_contexts(
    pull_request: &PullRequestPolicyWire,
) -> Result<Option<&CheckContextConnectionWire>, SourceDeliveryError> {
    let [commit] = pull_request.commits.nodes.as_slice() else {
        return Err(SourceDeliveryError::Rejected);
    };
    Ok(commit
        .commit
        .status_check_rollup
        .as_ref()
        .map(|rollup| &rollup.contexts))
}

fn classify_snapshot(
    pull_request: &PullRequestPolicyWire,
    contexts: &[CheckContextWire],
) -> PolicySnapshot {
    let mut evidence = classify_required_checks(contexts);
    if required_context_is_missing(pull_request, contexts) {
        evidence.checks = RequiredChecks::Pending;
    }
    if !evidence.failures.is_empty() {
        return PolicySnapshot {
            state: SourceReviewState::Open {
                checks: SourceChecks::Failed {
                    diagnostic: failure_diagnostic(evidence.failures),
                },
            },
            failed_job_ids: evidence.failed_job_ids,
            is_merge_queue_enabled: pull_request.is_merge_queue_enabled,
            head_update: head_update(pull_request),
            pull_request_ready: false,
        };
    }
    let policy_ready = merge_gate_ready(pull_request) || pull_request_ready(pull_request);
    let checks = match (policy_ready, evidence.checks) {
        (true, RequiredChecks::Absent) => SourceChecks::NotRequired,
        (true, RequiredChecks::Passed) => SourceChecks::Passed,
        _ => SourceChecks::Pending,
    };
    let pull_request_ready =
        !matches!(evidence.checks, RequiredChecks::Pending) && pull_request_ready(pull_request);
    PolicySnapshot {
        state: SourceReviewState::Open { checks },
        failed_job_ids: Vec::new(),
        is_merge_queue_enabled: pull_request.is_merge_queue_enabled,
        head_update: head_update(pull_request),
        pull_request_ready,
    }
}

fn required_context_is_missing(
    pull_request: &PullRequestPolicyWire,
    contexts: &[CheckContextWire],
) -> bool {
    let Some(required) = pull_request
        .base_ref
        .ref_update_rule
        .as_ref()
        .and_then(|rule| rule.required_status_check_contexts.as_ref())
    else {
        return false;
    };
    let observed = contexts
        .iter()
        .map(CheckContextWire::name)
        .collect::<BTreeSet<_>>();
    required
        .iter()
        .flatten()
        .filter(|name| !name.is_empty())
        .any(|name| !observed.contains(name.as_str()))
}

fn head_update(pull_request: &PullRequestPolicyWire) -> Option<HeadUpdate> {
    (!pull_request.is_merge_queue_enabled
        && !pull_request.is_draft
        && !pull_request.is_in_merge_queue
        && pull_request.mergeable == "MERGEABLE"
        && pull_request.merge_state_status == "BEHIND")
        .then(|| HeadUpdate(pull_request.id.clone()))
}

fn merge_gate_ready(pull_request: &PullRequestPolicyWire) -> bool {
    let state_ready = matches!(
        pull_request.merge_state_status.as_str(),
        "BEHIND" | "CLEAN" | "HAS_HOOKS" | "UNSTABLE"
    );
    !pull_request.is_draft
        && !pull_request.is_in_merge_queue
        && (pull_request.is_merge_queue_enabled
            || pull_request.mergeable == "MERGEABLE" && state_ready)
}

fn terminal_snapshot(
    pull_request: &PullRequestPolicyWire,
) -> Result<Option<PolicySnapshot>, SourceDeliveryError> {
    if pull_request.merged {
        return merged_snapshot(pull_request).map(Some);
    }
    let state = match pull_request.state.as_str() {
        "CLOSED" => Some(SourceReviewState::Closed),
        "OPEN" if pull_request.mergeable == "CONFLICTING" => Some(SourceReviewState::Conflict),
        "OPEN" if pull_request.merge_state_status == "DIRTY" => Some(SourceReviewState::Conflict),
        "OPEN" => None,
        _ => return Err(SourceDeliveryError::Rejected),
    };
    Ok(state.map(|state| PolicySnapshot {
        state,
        failed_job_ids: Vec::new(),
        is_merge_queue_enabled: false,
        head_update: None,
        pull_request_ready: false,
    }))
}

fn merged_snapshot(
    pull_request: &PullRequestPolicyWire,
) -> Result<PolicySnapshot, SourceDeliveryError> {
    let merge_revision = pull_request
        .merge_commit
        .as_ref()
        .map(|commit| commit.oid.clone())
        .filter(|revision| valid_revision(revision))
        .ok_or(SourceDeliveryError::Rejected)?;
    (pull_request.state == "MERGED")
        .then_some(PolicySnapshot {
            state: SourceReviewState::Merged { merge_revision },
            failed_job_ids: Vec::new(),
            is_merge_queue_enabled: false,
            head_update: None,
            pull_request_ready: false,
        })
        .ok_or(SourceDeliveryError::Rejected)
}

fn pull_request_ready(pull_request: &PullRequestPolicyWire) -> bool {
    !pull_request.is_draft
        && !pull_request.is_in_merge_queue
        && pull_request.mergeable == "MERGEABLE"
        && pull_request.merge_state_status == "BLOCKED"
        && matches!(
            pull_request.review_decision.as_deref(),
            Some("REVIEW_REQUIRED" | "CHANGES_REQUESTED")
        )
        && approval_handoff_policy_ready(&pull_request.base_ref)
}

fn approval_handoff_policy_ready(base_ref: &RefWire) -> bool {
    let Some(rule) = base_ref.ref_update_rule.as_ref() else {
        return false;
    };
    (rule.required_approving_review_count > 0 || rule.requires_code_owner_reviews)
        && !rule.requires_conversation_resolution
        && !rule.requires_linear_history
        && !rule.requires_signatures
}

fn waiting_snapshot() -> PolicySnapshot {
    PolicySnapshot {
        state: SourceReviewState::Open {
            checks: SourceChecks::Pending,
        },
        failed_job_ids: Vec::new(),
        is_merge_queue_enabled: false,
        head_update: None,
        pull_request_ready: false,
    }
}

fn bounded_text(value: &str, maximum: usize) -> String {
    const TRUNCATED: &str = "[diagnostic truncated]";
    if value.chars().count() <= maximum {
        return value.to_owned();
    }
    let mut text = value
        .chars()
        .take(maximum.saturating_sub(TRUNCATED.len()))
        .collect::<String>();
    text.extend(TRUNCATED.chars().take(maximum));
    text
}

fn log_tail(value: &str, maximum: usize) -> String {
    const TRUNCATED: &str = "[earlier log output truncated]\n";
    let clean = value
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect::<String>();
    if clean.chars().count() <= maximum {
        return clean;
    }
    let retain = maximum.saturating_sub(TRUNCATED.chars().count());
    let mut tail = clean.chars().rev().take(retain).collect::<Vec<_>>();
    tail.reverse();
    bounded_text(
        &format!("{TRUNCATED}{}", tail.into_iter().collect::<String>()),
        maximum,
    )
}
