#[cfg(unix)]
use std::path::{Path, PathBuf};

use super::*;
#[cfg(unix)]
use super::super::observation::test_support::{
    HEAD, OTHER_HEAD, assert_retryable_api, authority, receipt, shell_literal, write_executable,
};
#[cfg(unix)]
use openengine_cluster_testkit::assertions::AssertError;
use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::{Value, json};

fn user(login: &str) -> UserWire {
    UserWire {
        node_id: None,
        id: 1,
        kind: Some("User".to_owned()),
        login: login.to_owned(),
    }
}

#[cfg(unix)]
fn review_wire(head: &str) -> Value {
    json!({
        "number": 17,
        "base": {
            "ref": "main",
            "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "repo": {"full_name": "acme/project"}
        },
        "head": {
            "ref": "zeroshot/v2-test",
            "sha": head,
            "repo": {"full_name": "acme/project"}
        }
    })
}

#[cfg(unix)]
struct FeedbackPages {
    issue_comments: Value,
    reviews: Value,
    review_comments: Value,
}

#[cfg(unix)]
fn write_fixture(root: &Path, second_review_head: &str, pages: FeedbackPages) -> PathBuf {
    let program = root.join("gh-feedback-fixture");
    let source = format!(
        "#!/bin/sh\n\
         endpoint=$2\n\
         case \"$endpoint\" in\n\
         repos/acme/project/pulls/17)\n\
           count_file=\"$0.pull-count\"\n\
           count=0\n\
           if [ -f \"$count_file\" ]; then count=$(cat \"$count_file\"); fi\n\
           count=$((count + 1))\n\
           printf '%s' \"$count\" > \"$count_file\"\n\
           if [ \"$count\" -eq 1 ]; then printf '%s\\n' {}; else printf '%s\\n' {}; fi ;;\n\
         'repos/acme/project/issues/17/comments?per_page=100') printf '%s\\n' {} ;;\n\
         'repos/acme/project/pulls/17/reviews?per_page=100') printf '%s\\n' {} ;;\n\
         'repos/acme/project/pulls/17/comments?per_page=100') printf '%s\\n' {} ;;\n\
         repos/acme/project/collaborators/*/permission)\n\
           username=${{endpoint#repos/acme/project/collaborators/}}\n\
           username=${{username%/permission}}\n\
           printf '%s\\n' \"$username\" >> \"$0.permission-calls\"\n\
           if [ -f \"$0.$username.error\" ]; then cat \"$0.$username.error\" >&2; exit 1; fi\n\
           cat \"$0.$username.permission\" ;;\n\
         graphql) printf '%s\\n' {} ;;\n\
         *) exit 19 ;;\n\
         esac\n",
        shell_literal(&review_wire(HEAD).to_string()),
        shell_literal(&review_wire(second_review_head).to_string()),
        shell_literal(&pages.issue_comments.to_string()),
        shell_literal(&pages.reviews.to_string()),
        shell_literal(&pages.review_comments.to_string()),
        shell_literal(&json!([{"data":{"repository":{"nameWithOwner":"acme/project","pullRequest":{
            "number":17,"baseRefName":"main","headRefName":"zeroshot/v2-test","headRefOid":HEAD,
            "reviewThreads":{"totalCount":0,"pageInfo":{"hasNextPage":false,"endCursor":null},"nodes":[]}
        }}}}]).to_string()),
    );
    write_executable(&program, source);
    for surface in [
        &pages.issue_comments,
        &pages.reviews,
        &pages.review_comments,
    ] {
        if let Some(pages) = surface.as_array() {
            for entry in pages.iter().filter_map(Value::as_array).flatten() {
                let login = entry["user"]["login"].as_str().unwrap();
                set_permission(
                    &program,
                    login,
                    json!({"permission":"write", "user":entry["user"]}),
                );
            }
        }
    }
    program
}

fn set_permission(program: &Path, login: &str, value: Value) {
    std::fs::write(
        format!("{}.{}.permission", program.display(), login),
        value.to_string(),
    )
    .unwrap();
}

fn all_surfaces(authors: &[(u64, &str)]) -> FeedbackPages {
    let mut issues = Vec::new();
    let mut reviews = Vec::new();
    let mut inline = Vec::new();
    for (id, login) in authors {
        let user = json!({"id":id,"login":login,"type":"User"});
        issues.push(json!({
            "id":id*10,"updated_at":"one","user":user,"body":"Check the edge case.",
            "author_association":"MEMBER"
        }));
        for (offset, body) in [(1, Some("Review summary")), (2, None)] {
            reviews.push(json!({
                "id":id*10+offset,"state":"CHANGES_REQUESTED","submitted_at":"one",
                "commit_id":HEAD,"user":user,"body":body,"author_association":"OWNER"
            }));
        }
        for (offset, reply) in [(3, None), (4, Some(1))] {
            inline.push(json!({
                "id":id*10+offset,"updated_at":"one","user":user,"body":"Check this line.",
                "path":"src/lib.rs","line":17,"commit_id":HEAD,"in_reply_to_id":reply,
                "author_association":"COLLABORATOR"
            }));
        }
    }
    FeedbackPages {
        issue_comments: json!([[], issues]),
        reviews: json!([reviews, []]),
        review_comments: json!([inline]),
    }
}

fn permission_calls(program: &Path) -> Vec<String> {
    std::fs::read_to_string(format!("{}.permission-calls", program.display()))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn only_current_writers_can_supply_any_feedback_surface() {
    let root = tempfile::tempdir().unwrap();
    let program = write_fixture(
        root.path(),
        HEAD,
        all_surfaces(&[
            (1, "writer"),
            (2, "org-reader"),
            (3, "outsider"),
            (4, "admin"),
            (5, "bot"),
        ]),
    );
    set_permission(
        &program,
        "writer",
        json!({"permission":"write","role_name":"maintain","user":{"id":1,"login":"writer"}}),
    );
    set_permission(
        &program,
        "org-reader",
        json!({"permission":"read","role_name":"read","user":{"id":2,"login":"org-reader"}}),
    );
    set_permission(
        &program,
        "admin",
        json!({"permission":"admin","user":{"id":4,"login":"admin"}}),
    );
    set_permission(
        &program,
        "bot",
        json!({"permission":"none","user":{"id":5,"login":"bot"}}),
    );
    std::fs::write(
        format!("{}.outsider.error", program.display()),
        "gh: Not Found (HTTP 404)\n",
    )
    .unwrap();
    let authority = authority(program.clone(), root.path());
    let credential = SourceDeliveryCredential::token("test-token");
    let first = inspect(&authority, &receipt(), credential).await.unwrap();
    assert_eq!(first.items.len(), 10);
    assert!(
        first
            .items
            .iter()
            .all(|item| matches!(item.author.as_str(), "writer" | "admin"))
    );
    assert_eq!(permission_calls(&program).len(), 5);

    // An edit by an outsider stays invisible; a revoked writer cannot supply even old feedback.
    set_permission(
        &program,
        "writer",
        json!({"permission":"read","user":{"id":1,"login":"writer"}}),
    );
    let second = inspect(&authority, &receipt(), credential).await.unwrap();
    assert_eq!(second.items.len(), 5);
    assert!(second.items.iter().all(|item| item.author == "admin"));
    let calls = permission_calls(&program);
    for login in ["writer", "org-reader", "outsider", "admin", "bot"] {
        assert_eq!(calls.iter().filter(|called| *called == login).count(), 2);
    }
}

#[tokio::test]
async fn incomplete_or_mismatched_permissions_fail_the_entire_inspection() {
    let root = tempfile::tempdir().unwrap();
    let program = write_fixture(
        root.path(),
        HEAD,
        all_surfaces(&[(1, "writer"), (2, "unknown")]),
    );
    let authority = authority(program.clone(), root.path());
    for value in [
        json!({"user":{"id":2,"login":"unknown"}}),
        json!({"permission":"owner","user":{"id":2,"login":"unknown"}}),
        json!({"permission":null,"user":{"id":2,"login":"unknown"}}),
        json!({"permission":"write"}),
        json!({"permission":"write","user":{"login":"unknown"}}),
        json!({"permission":"write","user":{"id":99,"login":"unknown"}}),
    ] {
        set_permission(&program, "unknown", value.clone());
        assert!(
            inspect(
                &authority,
                &receipt(),
                SourceDeliveryCredential::token("test-token")
            )
            .await
            .is_err(),
            "accepted {value}"
        );
    }
    std::fs::write(
        format!("{}.unknown.permission", program.display()),
        "not json",
    )
    .unwrap();
    assert!(
        inspect(
            &authority,
            &receipt(),
            SourceDeliveryCredential::token("test-token")
        )
        .await
        .is_err()
    );
    for (diagnostic, status, retryable) in [
        ("gh: Bad credentials (HTTP 401)\n", Some(401), false),
        ("gh: Resource not accessible (HTTP 403)\n", Some(403), false),
        ("gh: API rate limit exceeded (HTTP 403)\n", Some(403), true),
        ("gh: Too Many Requests (HTTP 429)\n", Some(429), true),
        ("error connecting to api.github.com\n", None, true),
    ] {
        std::fs::write(format!("{}.unknown.error", program.display()), diagnostic).unwrap();
        let error = inspect(
            &authority,
            &receipt(),
            SourceDeliveryCredential::token("test-token"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.api_status(), status);
        assert_eq!(error.retryable_operation(), retryable, "{error}");
    }
}

#[test]
fn visible_feedback_surfaces_are_normalized_sorted_and_sanitized() {
    let items = collect_items(
        vec![IssueCommentWire {
            id: 9,
            updated_at: "2026-09-19T00:00:00Z".to_owned(),
            user: user("issue\0author"),
            body: Some("issue\0 body".to_owned()),
        }],
        vec![ReviewWire {
            id: 4,
            state: "CHANGES_REQUESTED".to_owned(),
            submitted_at: None,
            commit_id: None,
            user: user("reviewer"),
            body: None,
        }],
        vec![ReviewCommentWire {
            id: 2,
            updated_at: "2026-09-19T00:00:01Z".to_owned(),
            user: user("inline"),
            body: "fix this\nplease".to_owned(),
            path: Some("src/lib.rs".to_owned()),
            line: Some(17),
            original_line: None,
            commit_id: Some("a".repeat(40)),
            in_reply_to_id: Some(1),
        }],
    )
    .assert_value();
    let feedback = SourceReviewFeedback {
        conversations: Vec::new(),
        items: items.into_iter().map(|(_, item)| item).collect(),
    };

    assert_eq!(feedback.items.len(), 3);
    assert_eq!(feedback.items[0].key, "issue_comment:9");
    assert_eq!(feedback.items[1].key, "review:4");
    assert_eq!(feedback.items[2].key, "review_comment:2");
    assert_eq!(feedback.items[0].author, "issueauthor");
    assert_eq!(feedback.items[0].body, "issue body");
    assert!(feedback.items[1].body.contains("without a written summary"));
    assert!(
        feedback.items[2]
            .location
            .as_deref()
            .unwrap()
            .contains("line=17")
    );
    assert!(feedback.items.iter().all(|item| item.version.len() == 64));
}

#[test]
fn edits_change_the_private_checkpoint_version() {
    let first = item(FeedbackItemDraft {
        key: "issue_comment:1".to_owned(),
        provider_version: "one",
        author: "author".to_owned(),
        location: None,
        body: "body".to_owned(),
    });
    let edited = item(FeedbackItemDraft {
        key: "issue_comment:1".to_owned(),
        provider_version: "two",
        author: "author".to_owned(),
        location: None,
        body: "edited".to_owned(),
    });
    assert_ne!(first.version, edited.version);
}

#[cfg(unix)]
#[tokio::test]
async fn paginated_feedback_is_fenced_by_the_exact_review_identity() {
    let root = tempfile::tempdir().assert_value();
    let program = write_fixture(
        root.path(),
        HEAD,
        FeedbackPages {
            issue_comments: json!([[{
                "id": 9,
                "updated_at": "2026-09-19T00:00:00Z",
                "user": {"id":1,"login": "issue-author"},
                "body": "issue body"
            }], []]),
            reviews: json!([[], [{
                "id": 4,
                "state": "CHANGES_REQUESTED",
                "submitted_at": null,
                "commit_id": null,
                "user": {"id":2,"login": "reviewer"},
                "body": null
            }]]),
            review_comments: json!([[{
                "id": 2,
                "updated_at": "2026-09-19T00:00:01Z",
                "user": {"id":3,"login": "inline"},
                "body": "fix this",
                "path": "src/lib.rs",
                "line": null,
                "original_line": 17,
                "commit_id": HEAD,
                "in_reply_to_id": 1
            }]]),
        },
    );
    let feedback = inspect(
        &authority(program, root.path()),
        &receipt(),
        SourceDeliveryCredential::token("test-token"),
    )
    .await
    .assert_value();

    assert_eq!(
        feedback
            .items
            .iter()
            .map(|item| item.key.as_str())
            .collect::<Vec<_>>(),
        ["issue_comment:9", "review:4", "review_comment:2"]
    );
    assert!(feedback.items[1].body.contains("without a written summary"));
    assert!(
        feedback.items[2]
            .location
            .as_deref()
            .is_some_and(|location| location.contains("line=17"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn feedback_pagination_rejects_malformed_pages_and_identity_changes() {
    let changed_root = tempfile::tempdir().assert_value();
    let changed_program = write_fixture(
        changed_root.path(),
        OTHER_HEAD,
        FeedbackPages {
            issue_comments: json!([[]]),
            reviews: json!([[]]),
            review_comments: json!([[]]),
        },
    );
    let changed = inspect(
        &authority(changed_program, changed_root.path()),
        &receipt(),
        SourceDeliveryCredential::token("test-token"),
    )
    .await
    .assert_error();
    assert_retryable_api(&changed, "identity changed");

    let malformed_root = tempfile::tempdir().assert_value();
    let malformed_program = write_fixture(
        malformed_root.path(),
        HEAD,
        FeedbackPages {
            issue_comments: json!({"not": "pages"}),
            reviews: json!([[]]),
            review_comments: json!([[]]),
        },
    );
    let malformed = inspect(
        &authority(malformed_program, malformed_root.path()),
        &receipt(),
        SourceDeliveryCredential::token("test-token"),
    )
    .await
    .assert_error();
    assert!(matches!(malformed, SourceDeliveryError::Api(_)));
    assert!(!malformed.retryable_operation());
    assert!(malformed.to_string().contains("expected a sequence"));
}

#[tokio::test]
async fn app_bot_actions_on_every_surface_are_authorized_without_current_installation_or_permission()
 {
    let root = tempfile::tempdir().unwrap();
    let mut pages = all_surfaces(&[(71, "app-without-a-bot-suffix")]);
    for surface in [
        &mut pages.issue_comments,
        &mut pages.reviews,
        &mut pages.review_comments,
    ] {
        for entry in surface
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .flat_map(|page| page.as_array_mut().unwrap())
        {
            entry["user"]["type"] = json!("Bot");
        }
    }
    let program = write_fixture(root.path(), HEAD, pages);
    // Any permission or installation lookup is a fixture failure.
    std::fs::write(
        format!("{}.app-without-a-bot-suffix.error", program.display()),
        "gh: Not Found (HTTP 404)\n",
    )
    .unwrap();
    let authority = authority(program.clone(), root.path());
    for _ in 0..2 {
        let feedback = inspect(
            &authority,
            &receipt(),
            SourceDeliveryCredential::token("test-token"),
        )
        .await
        .unwrap();
        assert_eq!(feedback.items.len(), 5);
    }
    assert!(!std::path::Path::new(&format!("{}.permission-calls", program.display())).exists());
}
