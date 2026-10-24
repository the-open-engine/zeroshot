use std::collections::BTreeMap;
use std::sync::Mutex;
use openengine_cluster_testkit::assertions::AssertValue;
use super::*;

#[path = "transport_tests.rs"]
mod transport;

#[path = "validation_tests.rs"]
mod validation;

struct Metadata {
    values: BTreeMap<String, Value>,
    calls: Mutex<Vec<String>>,
}
#[async_trait::async_trait]
impl ReviewMetadata for Metadata {
    async fn get(&self, path: &str) -> Result<Value, ReviewResolveError> {
        self.calls.lock().unwrap().push(path.to_owned());
        self.values
            .get(path)
            .cloned()
            .ok_or(ReviewResolveError::Http(404))
    }
    async fn graphql(&self, query: &str, variables: &Value) -> Result<Value, ReviewResolveError> {
        assert!(query.contains("closingIssuesReferences(first: 100)"));
        assert_eq!(
            variables,
            &json!({"owner":"acme","name":"project","number":42})
        );
        self.get("graphql").await
    }
}
fn metadata(values: &[(&str, Value)]) -> Metadata {
    Metadata {
        values: values
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        calls: Mutex::new(Vec::new()),
    }
}
fn request(kind: &str) -> ReviewResolutionRequest {
    ReviewResolutionRequest {
        url: format!("https://github.com/acme/project/{kind}/42"),
        branch: None,
        revision: None,
    }
}
fn revision(byte: char) -> String {
    std::iter::repeat_n(byte, 40).collect()
}
fn pull() -> Value {
    json!({"data":{"repository":{"pullRequest":{
        "number":42,"url":"https://github.com/acme/project/pull/42","title":"Review this change","body":"A proposal",
        "baseRepository":{"nameWithOwner":"acme/project"},"baseRefName":"main","baseRefOid":revision('a'),
        "headRepository":{"nameWithOwner":"contributor/fork"},"headRefName":"feature/topic","headRefOid":revision('b'),
        "closingIssuesReferences":{"totalCount":0,"pageInfo":{"hasNextPage":false},"nodes":[]}
    }}}})
}

#[test]
fn canonical_url_parser_rejects_ambiguous_or_redirecting_locations() {
    for url in [
        "http://github.com/acme/project/issues/42",
        "https://github.com.evil.test/acme/project/issues/42",
        "https://user:token@github.com/acme/project/pull/42",
        "https://github.com/acme/project/issues/042",
        "https://github.com/acme/project/issues/0",
        "https://github.com/acme/project/issues/42?x=1",
        "https://github.com/acme/project/pull/42#discussion",
        "https://github.com/acme/project/pull/42/files",
        "https://github.com/acme%2fother/project/pull/42",
    ] {
        assert!(parse_url(url).is_err(), "accepted {url}");
    }
    assert_eq!(
        parse_url(&request("issues").url).assert_value().kind,
        ReviewTargetKind::Issue
    );
    assert_eq!(
        parse_url(&request("pull").url).assert_value().repository,
        "acme/project"
    );
}

#[tokio::test]
async fn issue_hydration_produces_ordinary_input_and_pins_the_default_branch() {
    let metadata = metadata(&[
        (
            "repos/acme/project/issues/42",
            json!({"number":42,"html_url":request("issues").url,"title":"Assess the proposal","body":null}),
        ),
        (
            "repos/acme/project",
            json!({"full_name":"acme/project","default_branch":"main"}),
        ),
        (
            "repos/acme/project/commits/main",
            json!({"sha":revision('a')}),
        ),
    ]);
    let result = crate::native_v2_review::resolve_review_with(request("issues"), &metadata)
        .await
        .assert_value();
    assert_eq!(
        result.initial_input,
        json!({"url":request("issues").url,"context":{"title":"Assess the proposal","body":"","reviewScope":{}}})
    );
    assert_eq!(result.source.revision.as_str(), revision('a'));
    assert_eq!(result.source.branch.as_str(), "main");
    assert!(result.initial_input.get("task").is_none());
}

#[tokio::test]
async fn fork_review_pins_both_sides_without_fetching_a_fork_token() {
    let metadata = metadata(&[("graphql", pull())]);
    let result = resolve_with(request("pull"), &metadata)
        .await
        .assert_value();
    assert_eq!(result.initial_input["context"]["linkedIssues"], json!([]));
    let scope = &result.initial_input["context"]["reviewScope"];
    assert_eq!(scope["headRepository"], "contributor/fork");
    assert_eq!(scope["baseRevision"], revision('a'));
    assert_eq!(scope["headRevision"], revision('b'));
    assert_eq!(result.source.repository.as_str(), "acme/project");
    assert_eq!(result.source.revision.as_str(), revision('b'));
    assert_eq!(*metadata.calls.lock().unwrap(), vec!["graphql"]);
}

#[tokio::test]
async fn mismatched_metadata_and_oversized_bodies_fail_without_truncation() {
    let mut changed = pull();
    changed["data"]["repository"]["pullRequest"]["number"] = json!(43);
    assert!(matches!(
        resolve_with(request("pull"), &metadata(&[("graphql", changed)])).await,
        Err(ReviewResolveError::Identity)
    ));
    let mut oversized = pull();
    oversized["data"]["repository"]["pullRequest"]["body"] = json!("x".repeat(MAX_BODY_BYTES + 1));
    assert!(matches!(
        resolve_with(request("pull"), &metadata(&[("graphql", oversized)])).await,
        Err(ReviewResolveError::TooLarge)
    ));
    let mut overridden = request("pull");
    overridden.branch = Some(SourceBranchId::new("main").assert_value());
    let empty = metadata(&[]);
    assert!(matches!(
        resolve_with(overridden, &empty).await,
        Err(ReviewResolveError::PullRequestOverrides)
    ));
    assert!(empty.calls.lock().unwrap().is_empty());
}

fn with_linked_issues(issues: Value) -> Value {
    let mut value = pull();
    let connection = &mut value["data"]["repository"]["pullRequest"]["closingIssuesReferences"];
    connection["totalCount"] = json!(issues.as_array().unwrap().len());
    connection["nodes"] = issues;
    value
}

fn linked_issue(repository: &str, number: u64) -> Value {
    json!({"number":number,"url":format!("https://github.com/{repository}/issues/{number}"),
        "title":"Acceptance criteria","body":"Cover this behavior completely.","repository":{"nameWithOwner":repository}})
}

#[tokio::test]
async fn linked_requirements_are_hydrated_and_sorted_across_repositories() {
    let value = with_linked_issues(json!([
        linked_issue("other/private", 8),
        linked_issue("acme/project", 7)
    ]));
    let result = resolve_with(request("pull"), &metadata(&[("graphql", value)]))
        .await
        .assert_value();
    let issues = result.initial_input["context"]["linkedIssues"]
        .as_array()
        .unwrap();
    assert_eq!(issues.len(), 2);
    assert_eq!(issues[0]["url"], "https://github.com/acme/project/issues/7");
    assert_eq!(issues[1]["body"], "Cover this behavior completely.");
}

#[tokio::test]
async fn incomplete_or_erroring_graphql_never_becomes_no_linked_issues() {
    let mut cases = vec![
        json!({"errors":[{"message":"denied"}],"data":null}),
        json!({"data":{"repository":null}}),
    ];
    for (path, value) in [
        (
            "/data/repository/pullRequest/closingIssuesReferences/pageInfo/hasNextPage",
            json!(true),
        ),
        (
            "/data/repository/pullRequest/closingIssuesReferences/totalCount",
            json!(1),
        ),
        (
            "/data/repository/pullRequest/closingIssuesReferences/nodes",
            json!([null]),
        ),
        (
            "/data/repository/pullRequest/closingIssuesReferences",
            Value::Null,
        ),
    ] {
        let mut candidate = pull();
        *candidate.pointer_mut(path).unwrap() = value;
        cases.push(candidate);
    }
    let mut partial = pull();
    partial["errors"] = json!([{"message":"one issue is unavailable"}]);
    cases.push(partial);
    for value in cases {
        assert!(matches!(
            resolve_with(request("pull"), &metadata(&[("graphql", value)])).await,
            Err(ReviewResolveError::LinkedIssues)
        ));
    }
}

#[tokio::test]
async fn linked_issue_identity_and_aggregate_size_fail_before_admission() {
    let mut foreign = linked_issue("acme/project", 7);
    foreign["url"] = json!("https://github.com/acme/project/pull/7");
    let duplicate = linked_issue("acme/project", 7);
    let mut oversized = linked_issue("acme/project", 8);
    oversized["body"] = json!("x".repeat(MAX_BODY_BYTES + 1));
    for issues in [
        json!([foreign]),
        json!([duplicate.clone(), duplicate]),
        json!([oversized]),
    ] {
        assert!(
            resolve_with(
                request("pull"),
                &metadata(&[("graphql", with_linked_issues(issues))])
            )
            .await
            .is_err()
        );
    }
}

#[tokio::test]
async fn both_pr_revisions_and_branch_names_are_validated_without_a_publisher() {
    for (field, value) in [
        ("baseRefOid", "not-a-sha"),
        ("headRefOid", "not-a-sha"),
        ("baseRefName", "bad..branch"),
        ("headRefName", "bad..branch"),
    ] {
        let mut candidate = pull();
        candidate["data"]["repository"]["pullRequest"][field] = json!(value);
        assert!(
            resolve_with(request("pull"), &metadata(&[("graphql", candidate)]))
                .await
                .is_err(),
            "accepted invalid {field}"
        );
    }
}

#[tokio::test]
async fn linked_issue_count_and_combined_response_bytes_are_bounded() {
    let too_many = (1..=101)
        .map(|number| linked_issue("acme/project", number))
        .collect::<Vec<_>>();
    assert!(matches!(
        resolve_with(
            request("pull"),
            &metadata(&[("graphql", with_linked_issues(json!(too_many)))])
        )
        .await,
        Err(ReviewResolveError::LinkedIssues)
    ));
    let large = (1..=3)
        .map(|number| {
            let mut issue = linked_issue("acme/project", number);
            issue["body"] = json!("x".repeat(750_000));
            issue
        })
        .collect::<Vec<_>>();
    assert!(matches!(
        resolve_with(
            request("pull"),
            &metadata(&[("graphql", with_linked_issues(json!(large)))])
        )
        .await,
        Err(ReviewResolveError::TooLarge)
    ));
}

#[tokio::test]
async fn case_variant_discussion_requests_retain_canonical_github_identity() {
    let issue_url = "https://github.com/Acme/Project/issues/42";
    let issue_metadata = metadata(&[
        (
            "repos/acme/project/issues/42",
            json!({"number":42,"html_url":issue_url,"title":"Issue","body":"Requirements"}),
        ),
        (
            "repos/acme/project",
            json!({"full_name":"Acme/Project","default_branch":"main"}),
        ),
        (
            "repos/acme/project/commits/main",
            json!({"sha":revision('a')}),
        ),
    ]);
    let issue = resolve_with(request("issues"), &issue_metadata)
        .await
        .assert_value();
    assert_eq!(issue.initial_input["url"], issue_url);
    assert_eq!(issue.source.repository.as_str(), "Acme/Project");
    let mut response = pull();
    response["data"]["repository"]["pullRequest"]["url"] =
        json!("https://github.com/Acme/Project/pull/42");
    response["data"]["repository"]["pullRequest"]["baseRepository"]["nameWithOwner"] =
        json!("Acme/Project");
    let pr = resolve_with(request("pull"), &metadata(&[("graphql", response)]))
        .await
        .assert_value();
    assert_eq!(
        pr.initial_input["url"],
        "https://github.com/Acme/Project/pull/42"
    );
    assert_eq!(pr.source.repository.as_str(), "Acme/Project");
    for url in [
        "https://github.com/Acme/Other/pull/42",
        "https://github.com/Acme/Project/issues/42",
        "https://github.com/Acme/Project/pull/43",
        "https://github.com/Acme/Project/pull/42?redirect=1",
    ] {
        assert!(!matches_discussion_url(
            &parse_url(&request("pull").url).assert_value(),
            url
        ));
    }
}
