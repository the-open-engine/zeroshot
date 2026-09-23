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
         *) exit 19 ;;\n\
         esac\n",
        shell_literal(&review_wire(HEAD).to_string()),
        shell_literal(&review_wire(second_review_head).to_string()),
        shell_literal(&pages.issue_comments.to_string()),
        shell_literal(&pages.reviews.to_string()),
        shell_literal(&pages.review_comments.to_string()),
    );
    write_executable(&program, source);
    program
}

#[test]
fn visible_feedback_surfaces_are_normalized_sorted_and_sanitized() {
    let feedback = collect_items(
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
                "user": {"login": "issue-author"},
                "body": "issue body"
            }], []]),
            reviews: json!([[], [{
                "id": 4,
                "state": "CHANGES_REQUESTED",
                "submitted_at": null,
                "commit_id": null,
                "user": {"login": "reviewer"},
                "body": null
            }]]),
            review_comments: json!([[{
                "id": 2,
                "updated_at": "2026-09-19T00:00:01Z",
                "user": {"login": "inline"},
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
        GitHubCredential("test-token"),
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
        GitHubCredential("test-token"),
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
        GitHubCredential("test-token"),
    )
    .await
    .assert_error();
    assert!(matches!(malformed, GitHubAuthorityError::Api(_)));
    assert!(!malformed.retryable_operation());
    assert!(malformed.to_string().contains("expected a sequence"));
}
