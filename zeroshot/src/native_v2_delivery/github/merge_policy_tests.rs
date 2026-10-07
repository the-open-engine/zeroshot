use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;

use super::*;

use super::super::unit::review;

fn pages(rule_pages: Vec<Vec<Value>>) -> Value {
    let total: usize = rule_pages.iter().map(Vec::len).sum();
    let page_count = rule_pages.len();
    let mut rule_id = 0;
    Value::Array(
        rule_pages
            .into_iter()
            .enumerate()
            .map(|(index, mut nodes)| {
                for node in &mut nodes {
                    node["id"] = json!(format!("RULE_{rule_id}"));
                    rule_id += 1;
                }
                json!({
                    "data": {
                        "repository": {
                            "nameWithOwner": "acme/project",
                            "mergeCommitAllowed": true,
                            "squashMergeAllowed": true,
                            "rebaseMergeAllowed": true,
                            "pullRequest": {
                                "id": "PR_node_17",
                                "number": 17,
                                "baseRefName": "main",
                                "headRefName": "zeroshot/v2-run",
                                "headRefOid": review().head_revision,
                                "isMergeQueueEnabled": false,
                                "baseRef": {
                                    "name": "main",
                                    "branchProtectionRule": null,
                                    "refUpdateRule": null,
                                    "rules": {
                                        "totalCount": total,
                                        "pageInfo": {
                                            "hasNextPage": index + 1 < page_count,
                                            "endCursor": format!("page_{index}"),
                                        },
                                        "nodes": nodes,
                                    },
                                },
                            },
                        },
                    },
                })
            })
            .collect(),
    )
}

fn base_ref(page: &mut Value) -> &mut Value {
    &mut page["data"]["repository"]["pullRequest"]["baseRef"]
}

fn method_rule(methods: Value) -> Value {
    json!({"type": "PULL_REQUEST", "parameters": {"allowedMergeMethods": methods}})
}

fn assert_permanent(error: &GitHubAuthorityError) {
    assert!(matches!(error, GitHubAuthorityError::Api(_)), "{error}");
    assert!(!error.retryable_operation(), "{error}");
}

#[test]
fn merge_policy_has_one_complete_rules_paginator() {
    let args = query_arguments(&review()).assert_value();
    assert_eq!(&args[..3], ["graphql", "--paginate", "--slurp"]);
    let query = &args[4];
    assert!(query.contains("query MergePolicy"));
    assert!(query.contains("rules(first: 100, after: $endCursor)"));
    assert_eq!(query.matches("pageInfo").count(), 1);
    assert!(query.contains("branchProtectionRule { requiresLinearHistory }"));
    assert!(query.contains("refUpdateRule { requiresLinearHistory }"));
    assert!(query.contains("allowedMergeMethods"));
}

#[test]
fn every_repository_method_combination_respects_all_linear_history_sources() {
    for mask in 0..8 {
        for source in ["none", "classic", "viewer", "ruleset"] {
            let mut input = pages(vec![if source == "ruleset" {
                vec![json!({"type": "REQUIRED_LINEAR_HISTORY"})]
            } else {
                vec![]
            }]);
            let repository = &mut input[0]["data"]["repository"];
            repository["mergeCommitAllowed"] = json!(mask & 1 != 0);
            repository["squashMergeAllowed"] = json!(mask & 2 != 0);
            repository["rebaseMergeAllowed"] = json!(mask & 4 != 0);
            if source == "classic" || source == "viewer" {
                let field = if source == "classic" {
                    "branchProtectionRule"
                } else {
                    "refUpdateRule"
                };
                base_ref(&mut input[0])[field] = json!({"requiresLinearHistory": true});
            }
            let expected = [
                (mask & 1 != 0 && source == "none", MergeMethod::Merge),
                (mask & 2 != 0, MergeMethod::Squash),
                (mask & 4 != 0, MergeMethod::Rebase),
            ]
            .into_iter()
            .find_map(|(enabled, method)| enabled.then_some(method));
            let result = classify(input, &review());
            if let Some(expected) = expected {
                assert_eq!(result.assert_value(), expected, "{mask} {source}");
            } else {
                assert_permanent(&result.assert_error());
            }
        }
    }
}

#[test]
fn paginates_more_than_one_hundred_rules_and_intersects_every_page() {
    let unrelated = vec![json!({"type": "NON_FAST_FORWARD", "parameters": null}); 100];
    let input = pages(vec![
        unrelated,
        vec![method_rule(json!(["SQUASH", "REBASE"]))],
        vec![method_rule(json!(["REBASE"]))],
    ]);
    assert_eq!(
        classify(input, &review()).assert_value(),
        MergeMethod::Rebase
    );

    let input = pages(vec![
        vec![json!({"type": "NON_FAST_FORWARD"}); 100],
        vec![json!({"type": "DELETION"})],
    ]);
    assert_eq!(
        classify(input, &review()).assert_value(),
        MergeMethod::Merge
    );
}

#[test]
fn method_restrictions_do_not_override_repository_or_linear_history() {
    let cases = [
        (
            vec![method_rule(json!(["SQUASH", "REBASE"]))],
            Some(MergeMethod::Squash),
        ),
        (
            vec![
                method_rule(json!(["MERGE"])),
                method_rule(json!(["REBASE"])),
            ],
            None,
        ),
        (vec![method_rule(json!([]))], None),
        (vec![method_rule(json!(["FUTURE_METHOD"]))], None),
        (
            vec![method_rule(json!(["FUTURE_METHOD", "REBASE"]))],
            Some(MergeMethod::Rebase),
        ),
        (
            vec![
                method_rule(Value::Null),
                json!({"type": "FUTURE_RULE", "parameters": null}),
            ],
            Some(MergeMethod::Merge),
        ),
    ];
    for (rules, expected) in cases {
        let result = classify(pages(vec![rules]), &review());
        if let Some(expected) = expected {
            assert_eq!(result.assert_value(), expected);
        } else {
            assert_permanent(&result.assert_error());
        }
    }

    let mut disabled = pages(vec![vec![method_rule(json!(["SQUASH"]))]]);
    disabled[0]["data"]["repository"]["squashMergeAllowed"] = json!(false);
    assert_permanent(&classify(disabled, &review()).assert_error());

    let mut linear = pages(vec![vec![method_rule(json!(["MERGE"]))]]);
    base_ref(&mut linear[0])["branchProtectionRule"] = json!({"requiresLinearHistory": true});
    base_ref(&mut linear[0])["refUpdateRule"] = json!({"requiresLinearHistory": false});
    assert_permanent(&classify(linear, &review()).assert_error());
}

#[test]
fn queue_owns_method_even_without_direct_methods_or_rules() {
    let mut input = pages(vec![vec![]]);
    let repository = &mut input[0]["data"]["repository"];
    for field in [
        "mergeCommitAllowed",
        "squashMergeAllowed",
        "rebaseMergeAllowed",
    ] {
        repository[field] = json!(false);
    }
    repository["pullRequest"]["isMergeQueueEnabled"] = json!(true);
    base_ref(&mut input[0])["rules"] = Value::Null;
    assert_eq!(
        classify(input, &review()).assert_value(),
        MergeMethod::Queue
    );
}

#[test]
fn policy_drift_between_pages_is_temporary() {
    for path in [
        "/data/repository/mergeCommitAllowed",
        "/data/repository/squashMergeAllowed",
        "/data/repository/rebaseMergeAllowed",
        "/data/repository/pullRequest/isMergeQueueEnabled",
    ] {
        let mut input = pages(vec![vec![json!({"type": "DELETION"})], vec![]]);
        let field = input[1].pointer_mut(path).assert_value();
        *field = json!(!field.as_bool().assert_value());
        let error = classify(input, &review()).assert_error();
        assert!(error.retryable_operation(), "{path}: {error}");
    }
    for field in ["branchProtectionRule", "refUpdateRule"] {
        let mut input = pages(vec![vec![json!({"type": "DELETION"})], vec![]]);
        base_ref(&mut input[1])[field] = json!({"requiresLinearHistory": true});
        assert!(
            classify(input, &review())
                .assert_error()
                .retryable_operation()
        );
    }
    let mut input = pages(vec![vec![json!({"type": "DELETION"})], vec![]]);
    base_ref(&mut input[1])["rules"]["totalCount"] = json!(2);
    assert!(
        classify(input, &review())
            .assert_error()
            .retryable_operation()
    );
}

#[test]
fn every_page_must_match_the_admitted_identity() {
    for page in [0, 1] {
        for (path, value) in [
            ("/data/repository/nameWithOwner", json!("other/project")),
            ("/data/repository/pullRequest/number", json!(18)),
            ("/data/repository/pullRequest/baseRefName", json!("other")),
            ("/data/repository/pullRequest/baseRef/name", json!("other")),
            ("/data/repository/pullRequest/headRefName", json!("other")),
            (
                "/data/repository/pullRequest/headRefOid",
                json!("cccccccccccccccccccccccccccccccccccccccc"),
            ),
        ] {
            let mut input = pages(vec![vec![json!({"type": "DELETION"})], vec![]]);
            *input[page].pointer_mut(path).assert_value() = value;
            let error = classify(input, &review()).assert_error();
            assert!(
                matches!(error, GitHubAuthorityError::Identity(_)),
                "{path}: {error}"
            );
            assert!(!error.retryable_operation());
        }
    }
}

#[test]
fn incomplete_repeated_and_inconsistent_pages_fail_closed() {
    let original = pages(vec![
        vec![json!({"type": "DELETION"})],
        vec![method_rule(json!(["SQUASH"]))],
    ]);
    let mut missing_page = original.clone();
    missing_page.as_array_mut().assert_value().pop();
    let mut extra_page = original.clone();
    extra_page
        .as_array_mut()
        .assert_value()
        .push(original[1].clone());
    let mut repeated_cursor = original.clone();
    base_ref(&mut repeated_cursor[1])["rules"]["pageInfo"]["endCursor"] = json!("page_0");
    let mut missing_cursor = original.clone();
    base_ref(&mut missing_cursor[0])["rules"]["pageInfo"]["endCursor"] = Value::Null;
    let mut repeated_rule = original.clone();
    base_ref(&mut repeated_rule[1])["rules"]["nodes"][0]["id"] = json!("RULE_0");
    let mut omitted_rule = original.clone();
    base_ref(&mut omitted_rule[1])["rules"]["nodes"] = json!([]);
    let mut wrong_count = original.clone();
    for page in wrong_count.as_array_mut().assert_value() {
        base_ref(page)["rules"]["totalCount"] = json!(1);
    }
    let mut no_progress = original.clone();
    base_ref(&mut no_progress[0])["rules"]["nodes"] = json!([]);

    for input in [
        missing_page,
        extra_page,
        repeated_cursor,
        missing_cursor,
        repeated_rule,
        omitted_rule,
        wrong_count,
        no_progress,
        json!([]),
    ] {
        assert_permanent(&classify(input, &review()).assert_error());
    }
}

#[test]
fn missing_or_malformed_schema_is_permanent_but_nullable_fields_are_supported() {
    for path in [
        "/data/repository/pullRequest/baseRef/branchProtectionRule",
        "/data/repository/pullRequest/baseRef/refUpdateRule",
        "/data/repository/pullRequest/baseRef/rules",
        "/data/repository/pullRequest/baseRef/rules/pageInfo",
        "/data/repository/pullRequest/baseRef/rules/pageInfo/endCursor",
    ] {
        let mut input = pages(vec![vec![]]);
        let (parent, field) = path.rsplit_once('/').assert_value();
        input[0]
            .pointer_mut(parent)
            .assert_value()
            .as_object_mut()
            .assert_value()
            .remove(field);
        assert_permanent(&classify(input, &review()).assert_error());
    }
    for rules in [
        Value::Null,
        json!({"totalCount": 1, "pageInfo": {"hasNextPage": false, "endCursor": null}, "nodes": [null]}),
        json!({"totalCount": 1, "pageInfo": {"hasNextPage": false, "endCursor": null},
            "nodes": [{"id": "RULE", "type": "PULL_REQUEST", "parameters": null}]}),
        json!({"totalCount": 1, "pageInfo": {"hasNextPage": false, "endCursor": null},
            "nodes": [{"id": "RULE", "type": "PULL_REQUEST", "parameters": {}}]}),
    ] {
        let mut input = pages(vec![vec![]]);
        base_ref(&mut input[0])["rules"] = rules;
        assert_permanent(&classify(input, &review()).assert_error());
    }
    let mut input = pages(vec![vec![method_rule(Value::Null)]]);
    base_ref(&mut input[0])["rules"]["pageInfo"]["endCursor"] = Value::Null;
    assert_eq!(
        classify(input, &review()).assert_value(),
        MergeMethod::Merge
    );
}
