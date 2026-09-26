use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value;

use super::{GitHubAuthorityError, GitHubReviewReceipt};

const MERGE_POLICY_QUERY: &str = r#"
query MergePolicy($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    nameWithOwner
    mergeCommitAllowed
    squashMergeAllowed
    rebaseMergeAllowed
    pullRequest(number: $number) {
      id
      number
      baseRefName
      headRefName
      headRefOid
      isMergeQueueEnabled
      baseRef {
        name
        branchProtectionRule { requiresLinearHistory }
        refUpdateRule { requiresLinearHistory }
        rules(first: 100, after: $endCursor) {
          totalCount
          pageInfo { hasNextPage endCursor }
          nodes {
            id
            type
            parameters { ... on PullRequestParameters { allowedMergeMethods } }
          }
        }
      }
    }
  }
}
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MergeMethod {
    Queue,
    Merge,
    Squash,
    Rebase,
}

#[derive(Deserialize)]
struct QueryPage {
    data: QueryData,
}

#[derive(Deserialize)]
struct QueryData {
    repository: Repository,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repository {
    name_with_owner: String,
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    pull_request: PullRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequest {
    id: String,
    number: u64,
    base_ref_name: String,
    head_ref_name: String,
    head_ref_oid: String,
    is_merge_queue_enabled: bool,
    base_ref: BaseRef,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaseRef {
    name: String,
    #[serde(deserialize_with = "Option::deserialize")]
    branch_protection_rule: Option<LinearHistory>,
    #[serde(deserialize_with = "Option::deserialize")]
    ref_update_rule: Option<LinearHistory>,
    #[serde(deserialize_with = "Option::deserialize")]
    rules: Option<Rules>,
}

#[derive(Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct LinearHistory {
    requires_linear_history: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rules {
    total_count: usize,
    page_info: PageInfo,
    nodes: Vec<Rule>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct Rule {
    id: String,
    #[serde(flatten)]
    restriction: Restriction,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum Restriction {
    RequiredLinearHistory,
    PullRequest {
        parameters: PullRequestParameters,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRequestParameters {
    #[serde(deserialize_with = "Option::deserialize")]
    allowed_merge_methods: Option<Vec<String>>,
}

pub(super) fn query_arguments(
    review: &GitHubReviewReceipt,
) -> Result<Vec<String>, GitHubAuthorityError> {
    super::review_query_arguments(review, MERGE_POLICY_QUERY)
}

pub(super) fn classify(
    value: Value,
    review: &GitHubReviewReceipt,
) -> Result<MergeMethod, GitHubAuthorityError> {
    let pages: Vec<QueryPage> = serde_json::from_value(value)
        .map_err(|error| invalid(format!("invalid response: {error}")))?;
    let first = validate_identity_and_policy(&pages, review)?;
    if first.pull_request.is_merge_queue_enabled {
        return Ok(MergeMethod::Queue);
    }

    let base = &first.pull_request.base_ref;
    let linear = base
        .branch_protection_rule
        .as_ref()
        .is_some_and(|rule| rule.requires_linear_history)
        || base
            .ref_update_rule
            .as_ref()
            .is_some_and(|rule| rule.requires_linear_history);
    let mut allowed = [
        first.merge_commit_allowed && !linear,
        first.squash_merge_allowed,
        first.rebase_merge_allowed,
    ];
    apply_rules(&pages, rules(first)?.total_count, &mut allowed)?;

    // GitHub matches active rules, including organization rulesets. Every rule must allow
    // the selected method; delivery does not infer or exercise actor bypass privileges.
    allowed
        .into_iter()
        .zip([MergeMethod::Merge, MergeMethod::Squash, MergeMethod::Rebase])
        .find_map(|(enabled, method)| enabled.then_some(method))
        .ok_or_else(|| {
            GitHubAuthorityError::api(
                None,
                "No merge method is allowed by both repository settings and base-branch protection/rulesets. \
                 Enable a compatible merge method or use a merge queue before retrying delivery.",
            )
        })
}

fn validate_identity_and_policy<'a>(
    pages: &'a [QueryPage],
    review: &GitHubReviewReceipt,
) -> Result<&'a Repository, GitHubAuthorityError> {
    let first = &pages
        .first()
        .ok_or_else(|| invalid("no response pages"))?
        .data
        .repository;
    for page in pages {
        let repository = &page.data.repository;
        require_identity(repository, review)?;
        if !same_policy(first, repository) {
            return Err(
                invalid("repository or branch policy changed during pagination").temporary(),
            );
        }
    }
    Ok(first)
}

fn apply_rules(
    pages: &[QueryPage],
    expected_count: usize,
    allowed: &mut [bool; 3],
) -> Result<(), GitHubAuthorityError> {
    let mut cursors = BTreeSet::new();
    let mut rule_ids = BTreeSet::new();
    for (index, page) in pages.iter().enumerate() {
        let rules = rules(&page.data.repository)?;
        if rules.total_count != expected_count {
            return Err(invalid("branch rules changed during pagination").temporary());
        }
        validate_page(rules, index + 1 < pages.len(), &mut cursors)?;
        for rule in &rules.nodes {
            if rule.id.is_empty() || !rule_ids.insert(&rule.id) {
                return Err(invalid(
                    "branch-rule pagination repeated an empty or previous rule ID",
                ));
            }
            apply_restriction(&rule.restriction, allowed);
        }
    }
    if rule_ids.len() != expected_count {
        return Err(invalid(
            "branch-rule count does not match the complete paginated response",
        ));
    }
    Ok(())
}

fn validate_page<'a>(
    rules: &'a Rules,
    has_next_page: bool,
    cursors: &mut BTreeSet<&'a str>,
) -> Result<(), GitHubAuthorityError> {
    if rules.page_info.has_next_page != has_next_page {
        return Err(invalid("incomplete branch-rule pagination"));
    }
    match rules.page_info.end_cursor.as_deref() {
        Some(cursor) if cursor.is_empty() || !cursors.insert(cursor) => {
            return Err(invalid(
                "branch-rule pagination repeated an empty or previous cursor",
            ));
        }
        None if has_next_page => {
            return Err(invalid("branch-rule pagination omitted its next cursor"));
        }
        _ => {}
    }
    if has_next_page && rules.nodes.is_empty() {
        return Err(invalid("branch-rule pagination made no progress"));
    }
    Ok(())
}

fn apply_restriction(restriction: &Restriction, allowed: &mut [bool; 3]) {
    match restriction {
        Restriction::RequiredLinearHistory => allowed[0] = false,
        Restriction::PullRequest { parameters } => {
            if let Some(methods) = &parameters.allowed_merge_methods {
                for (enabled, name) in allowed.iter_mut().zip(["MERGE", "SQUASH", "REBASE"]) {
                    *enabled &= methods.iter().any(|method| method == name);
                }
            }
        }
        Restriction::Other => {}
    }
}

fn rules(repository: &Repository) -> Result<&Rules, GitHubAuthorityError> {
    repository
        .pull_request
        .base_ref
        .rules
        .as_ref()
        .ok_or_else(|| invalid("GitHub did not return the base-branch rules"))
}

fn require_identity(
    repository: &Repository,
    review: &GitHubReviewReceipt,
) -> Result<(), GitHubAuthorityError> {
    let pull_request = &repository.pull_request;
    if repository.name_with_owner != review.repository
        || pull_request.number.to_string() != review.review_id
        || pull_request.base_ref_name != review.target_branch
        || pull_request.base_ref.name != review.target_branch
        || pull_request.head_ref_name != review.head_branch
        || pull_request.head_ref_oid != review.head_revision
        || pull_request.id.is_empty()
    {
        return Err(GitHubAuthorityError::identity(
            "Merge policy no longer matches the admitted pull request, base branch, and exact head.",
        ));
    }
    Ok(())
}

fn same_policy(first: &Repository, current: &Repository) -> bool {
    let left = &first.pull_request;
    let right = &current.pull_request;
    first.merge_commit_allowed == current.merge_commit_allowed
        && first.squash_merge_allowed == current.squash_merge_allowed
        && first.rebase_merge_allowed == current.rebase_merge_allowed
        && left.id == right.id
        && left.is_merge_queue_enabled == right.is_merge_queue_enabled
        && left.base_ref.branch_protection_rule == right.base_ref.branch_protection_rule
        && left.base_ref.ref_update_rule == right.base_ref.ref_update_rule
}

fn invalid(detail: impl std::fmt::Display) -> GitHubAuthorityError {
    GitHubAuthorityError::api(
        None,
        format!("Cannot determine a permitted merge method: {detail}."),
    )
}

#[cfg(test)]
#[path = "merge_policy_tests.rs"]
mod tests;
