//! One bounded snapshot of PR source identity and its formal linked requirements.
use super::*;

const PR_QUERY: &str = r#"query ZeroshotPrReview($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      number url title body
      baseRefName baseRefOid headRefName headRefOid
      baseRepository { nameWithOwner }
      headRepository { nameWithOwner }
      closingIssuesReferences(first: 100) {
        totalCount
        pageInfo { hasNextPage }
        nodes { number url title body repository { nameWithOwner } }
      }
    }
  }
}
"#;

pub(super) async fn resolve_pull(
    metadata: &dyn ReviewMetadata,
    target: &ReviewTarget,
) -> Result<ResolvedParts, ReviewResolveError> {
    let (owner, name) = target
        .repository
        .split_once('/')
        .ok_or(ReviewResolveError::Identity)?;
    let value = metadata
        .graphql(
            PR_QUERY,
            &json!({"owner":owner,"name":name,"number":target.number}),
        )
        .await?;
    if serde_json::to_vec(&value)
        .map_err(|_| ReviewResolveError::Identity)?
        .len()
        > MAX_API_BYTES
    {
        return Err(ReviewResolveError::TooLarge);
    }
    let response: Response =
        serde_json::from_value(value).map_err(|_| ReviewResolveError::LinkedIssues)?;
    if !response.errors.is_empty() {
        return Err(ReviewResolveError::LinkedIssues);
    }
    let pull = response
        .data
        .ok_or(ReviewResolveError::LinkedIssues)?
        .repository
        .ok_or(ReviewResolveError::LinkedIssues)?
        .pull_request
        .ok_or(ReviewResolveError::LinkedIssues)?;
    prepare_pull(pull, target)
}

fn prepare_pull(pull: Pull, target: &ReviewTarget) -> Result<ResolvedParts, ReviewResolveError> {
    if pull.number != target.number
        || !matches_discussion_url(target, &pull.url)
        || !pull
            .base_repository
            .name_with_owner
            .eq_ignore_ascii_case(&target.repository)
    {
        return Err(ReviewResolveError::Identity);
    }
    validate_text(&pull.title, &pull.body)?;
    canonical_repository(&pull.base_repository.name_with_owner)?;
    canonical_repository(&pull.head_repository.name_with_owner)?;
    let source = source(&pull)?;
    let scope = json!({"baseRepository":pull.base_repository.name_with_owner,"baseBranch":pull.base_ref_name,"baseRevision":pull.base_ref_oid,
        "headRepository":pull.head_repository.name_with_owner,"headBranch":pull.head_ref_name,"headRevision":pull.head_ref_oid});
    let issues = linked_issues(pull.closing_issues_references)?;
    Ok((
        pull.url,
        json!({"title":pull.title,"body":pull.body,"reviewScope":scope,"linkedIssues":issues}),
        source,
    ))
}

fn source(pull: &Pull) -> Result<ResolvedSource, ReviewResolveError> {
    Ok(ResolvedSource {
        repository: SourceRepositoryId::new(pull.base_repository.name_with_owner.clone())
            .map_err(|_| ReviewResolveError::Identity)?,
        branch: pull.base_ref_name.clone(),
        revision: pull.head_ref_oid.clone(),
        remote: None,
    })
}

fn linked_issues(connection: IssueConnection) -> Result<Vec<Value>, ReviewResolveError> {
    if connection.page_info.has_next_page
        || connection.nodes.len() > 100
        || connection.total_count != connection.nodes.len()
    {
        return Err(ReviewResolveError::LinkedIssues);
    }
    let mut issues = std::collections::BTreeMap::new();
    for issue in connection.nodes {
        validate_issue(&issue)?;
        if issues
            .insert(
                issue.url.clone(),
                json!({"url":issue.url,"title":issue.title,"body":issue.body}),
            )
            .is_some()
        {
            return Err(ReviewResolveError::LinkedIssues);
        }
    }
    Ok(issues.into_values().collect())
}

fn validate_issue(issue: &LinkedIssue) -> Result<(), ReviewResolveError> {
    let identity = parse_url(&issue.url)?;
    if identity.kind != ReviewTargetKind::Issue
        || identity.number != issue.number
        || !identity
            .repository
            .eq_ignore_ascii_case(&issue.repository.name_with_owner)
    {
        return Err(ReviewResolveError::Identity);
    }
    validate_text(&issue.title, &issue.body)?;
    Ok(())
}

#[derive(Deserialize)]
struct Response {
    data: Option<Data>,
    #[serde(default)]
    errors: Vec<Value>,
}
#[derive(Deserialize)]
struct Data {
    repository: Option<PullRepository>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PullRepository {
    pull_request: Option<Pull>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pull {
    number: u64,
    url: String,
    title: String,
    body: String,
    base_ref_name: SourceBranchId,
    base_ref_oid: SourceRevisionId,
    head_ref_name: SourceBranchId,
    head_ref_oid: SourceRevisionId,
    base_repository: RepositoryName,
    head_repository: RepositoryName,
    closing_issues_references: IssueConnection,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryName {
    name_with_owner: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueConnection {
    total_count: usize,
    page_info: PageInfo,
    nodes: Vec<LinkedIssue>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
}
#[derive(Deserialize)]
struct LinkedIssue {
    number: u64,
    url: String,
    title: String,
    body: String,
    repository: RepositoryName,
}
