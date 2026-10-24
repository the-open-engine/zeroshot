use super::*;

fn issue() -> Value {
    json!({"number":42,"html_url":request("issues").url,"title":"Issue","body":"Acceptance criteria"})
}

fn issue_metadata(issue: Value, repository: Value, commit: Value) -> Metadata {
    metadata(&[
        ("repos/acme/project/issues/42", issue),
        ("repos/acme/project", repository),
        ("repos/acme/project/commits/main", commit),
    ])
}

fn repository() -> Value {
    json!({"full_name":"acme/project","default_branch":"main"})
}

#[tokio::test]
async fn issue_checkout_overrides_pin_the_requested_commit_and_escape_branch_names() {
    for explicit_revision in [false, true] {
        let mut selected = request("issues");
        selected.branch = Some(SourceBranchId::new("release/harden").assert_value());
        if explicit_revision {
            selected.revision = Some(SourceRevisionId::new(revision('b')).assert_value());
        }
        let reference = if explicit_revision {
            revision('b')
        } else {
            "release%2Fharden".into()
        };
        let path = format!("repos/acme/project/commits/{reference}");
        let evidence = metadata(&[
            ("repos/acme/project/issues/42", issue()),
            ("repos/acme/project", repository()),
            (&path, json!({"sha":revision('b')})),
        ]);
        let prepared = resolve_with(selected, &evidence).await.assert_value();
        assert_eq!(prepared.source.branch.as_str(), "release/harden");
        assert_eq!(prepared.source.revision.as_str(), revision('b'));
        assert!(evidence.calls.lock().unwrap().contains(&path));
    }
}

#[tokio::test]
async fn issue_metadata_rejects_wrong_discussion_and_invalid_source_identity() {
    for (part, field, invalid) in [
        ("issue", "number", json!(43)),
        (
            "issue",
            "html_url",
            json!("https://github.com/other/project/issues/42"),
        ),
        (
            "issue",
            "pull_request",
            json!({"url":"https://api.github.com/repos/acme/project/pulls/42"}),
        ),
        ("repository", "full_name", json!("other/project")),
        ("repository", "default_branch", json!("-invalid")),
        ("commit", "sha", json!("not-a-sha")),
    ] {
        let (mut issue, mut repository, mut commit) =
            (issue(), repository(), json!({"sha":revision('a')}));
        match part {
            "issue" => issue[field] = invalid,
            "repository" => repository[field] = invalid,
            _ => commit[field] = invalid,
        }
        let fixture = issue_metadata(issue, repository, commit);
        assert!(
            matches!(
                resolve_with(request("issues"), &fixture).await,
                Err(ReviewResolveError::Identity)
            ),
            "accepted invalid {part}.{field}"
        );
    }
}

#[tokio::test]
async fn issue_revision_override_cannot_accept_a_different_returned_commit() {
    let expected = revision('b');
    let mut selected = request("issues");
    selected.revision = Some(SourceRevisionId::new(&expected).assert_value());
    let path = format!("repos/acme/project/commits/{expected}");
    let fixture = metadata(&[
        ("repos/acme/project/issues/42", issue()),
        ("repos/acme/project", repository()),
        (&path, json!({"sha":revision('a')})),
    ]);
    assert!(matches!(
        resolve_with(selected, &fixture).await,
        Err(ReviewResolveError::Identity)
    ));
}

#[tokio::test]
async fn malformed_oversized_and_unavailable_issue_metadata_fail_before_admission() {
    assert!(matches!(
        resolve_with(request("issues"), &metadata(&[])).await,
        Err(ReviewResolveError::Http(404))
    ));
    for value in [
        Value::Null,
        json!({"number":42}),
        json!({"unexpected":"x".repeat(MAX_API_BYTES)}),
    ] {
        let oversized = value.get("unexpected").is_some();
        let fixture = issue_metadata(value, repository(), json!({"sha":revision('a')}));
        let result = resolve_with(request("issues"), &fixture).await;
        if oversized {
            assert!(matches!(result, Err(ReviewResolveError::TooLarge)));
        } else {
            assert!(matches!(result, Err(ReviewResolveError::Identity)));
        }
    }
    for (field, value) in [
        ("title", String::new()),
        ("title", "x".repeat(4097)),
        ("body", "x".repeat(MAX_BODY_BYTES + 1)),
    ] {
        let mut value_issue = issue();
        value_issue[field] = json!(value);
        let fixture = issue_metadata(value_issue, repository(), json!({"sha":revision('a')}));
        assert!(matches!(
            resolve_with(request("issues"), &fixture).await,
            Err(ReviewResolveError::TooLarge)
        ));
    }
}

#[tokio::test]
async fn pr_and_linked_issue_identity_must_match_the_observed_discussion() {
    for (pointer, invalid) in [
        (
            "/data/repository/pullRequest/baseRepository/nameWithOwner",
            json!("other/project"),
        ),
        (
            "/data/repository/pullRequest/headRepository/nameWithOwner",
            json!("owner/repo/extra"),
        ),
        (
            "/data/repository/pullRequest/url",
            json!("https://github.com/acme/project/issues/42"),
        ),
        (
            "/data/repository/pullRequest/url",
            json!("https://github.com/acme/project/pull/42/../42"),
        ),
    ] {
        let mut value = pull();
        *value.pointer_mut(pointer).unwrap() = invalid;
        assert!(
            resolve_with(request("pull"), &metadata(&[("graphql", value)]))
                .await
                .is_err()
        );
    }
    for (field, value) in [
        ("number", json!(8)),
        ("repository", json!({"nameWithOwner":"other/project"})),
        ("title", json!("")),
    ] {
        let mut issue = linked_issue("acme/project", 7);
        issue[field] = value;
        assert!(
            resolve_with(
                request("pull"),
                &metadata(&[("graphql", with_linked_issues(json!([issue])))])
            )
            .await
            .is_err()
        );
    }
}

#[tokio::test]
async fn absent_or_malformed_graphql_nodes_never_produce_a_prepared_review() {
    for value in [
        Value::Null,
        json!({}),
        json!({"data":{"repository":{"pullRequest":null}}}),
        json!({"data":{"repository":{"pullRequest":{"number":42}}}}),
    ] {
        assert!(matches!(
            resolve_with(request("pull"), &metadata(&[("graphql", value)])).await,
            Err(ReviewResolveError::LinkedIssues)
        ));
    }
}

#[test]
fn malformed_or_non_discussion_urls_fail_before_metadata_lookup() {
    for url in [
        "github.com/acme/project/issues/42".to_owned(),
        "https://github.com/acme/project/discussions/42".to_owned(),
        "https://github.com/acme/project/pull/not-a-number".to_owned(),
        "https://github.com/acme/project/pull/18446744073709551616".to_owned(),
        "https://github.com:444/acme/project/pull/42".to_owned(),
        "https://:token@github.com/acme/project/pull/42".to_owned(),
        format!("https://github.com/{}/project/issues/42", "x".repeat(2048)),
    ] {
        assert!(
            matches!(parse_url(&url), Err(ReviewResolveError::InvalidUrl)),
            "accepted {url}"
        );
    }
}
