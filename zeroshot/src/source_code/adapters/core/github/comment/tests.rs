#![cfg(unix)]
use std::time::Duration;

use openengine_cluster_testkit::fixture::write_executable;
use super::*;

fn authority(root: &std::path::Path, program: &str) -> GhCliDeliveryAuthority {
    GhCliDeliveryAuthority::new(super::super::GhCliAuthorityConfig {
        gh_program: root.join(program),
        home_directory: root.to_owned(),
        api_deadline: Duration::from_secs(5),
        ..super::super::GhCliAuthorityConfig::hosted(root.to_owned())
    })
}

#[tokio::test]
async fn report_json_uses_stdin_and_preserves_large_utf8_body() {
    let root = tempfile::tempdir().unwrap();
    let script = r##"#!/usr/bin/python3
import json,sys
assert sys.argv[1:] == ['api','repos/acme/project/issues/17/comments','--method','POST','--input','-']
body=json.load(sys.stdin)['body']
print(json.dumps({'id':23,'html_url':'https://github.com/acme/project/issues/17#issuecomment-23','issue_url':'https://api.github.com/repos/acme/project/issues/17','body':body,'user':{'node_id':'ACTOR_ID'}}))
"##;
    write_executable(&root.path().join("gh"), script, 0o700).unwrap();
    let authority = authority(root.path(), "gh");
    let destination = comment_destination("https://github.com/acme/project/issues/17").unwrap();
    let body = "review \"quoted\" β\n".repeat(2000);
    let comment = authority
        .create_comment(
            &destination,
            &body,
            SourceDeliveryCredential::token("private-canary"),
        )
        .await
        .unwrap();
    assert_eq!(comment.body, body);
    assert_eq!(comment.author, "ACTOR_ID");
    assert_eq!(comment.id, "23");
}

#[tokio::test]
async fn failed_post_preserves_http_authentication_and_redacts_credentials() {
    let root = tempfile::tempdir().unwrap();
    write_executable(&root.path().join("gh"), "#!/bin/sh\n/bin/cat >/dev/null\nprintf 'HTTP 401: Bad credentials %s\\n' \"$GH_TOKEN\" >&2\nexit 1\n", 0o700).unwrap();
    let destination = comment_destination("https://github.com/acme/project/issues/17").unwrap();
    let error = authority(root.path(), "gh")
        .create_comment(
            &destination,
            "report",
            SourceDeliveryCredential::token("private-canary"),
        )
        .await
        .unwrap_err();
    assert_eq!(error.api_status(), Some(401));
    assert!(error.authentication_failed());
    assert!(!error.to_string().contains("private-canary"));
}

#[tokio::test]
async fn comment_pages_keep_protocol_bytes_larger_than_diagnostic_limit() {
    let root = tempfile::tempdir().unwrap();
    let script = r##"#!/usr/bin/python3
import json
print(json.dumps([{'id':n,'html_url':f'https://github.com/acme/project/issues/17#issuecomment-{n}','issue_url':'https://api.github.com/repos/acme/project/issues/17','body':'a'*4096,'user':{'node_id':'ACTOR_ID'}} for n in range(1,101)]))
"##;
    write_executable(&root.path().join("gh"), script, 0o700).unwrap();
    let destination = comment_destination("https://github.com/acme/project/issues/17").unwrap();
    let page = authority(root.path(), "gh")
        .list_comments(
            &destination,
            1,
            SourceDeliveryCredential::token("private-canary"),
        )
        .await
        .unwrap();
    assert_eq!(page.comments.len(), 100);
    assert!(page.has_more);
}

#[test]
fn comment_identity_cannot_cross_discussions() {
    let destination = comment_destination("https://github.com/acme/project/pull/17").unwrap();
    let wire = json!({"id":23,"html_url":"https://github.com/acme/project/issues/18#issuecomment-23","issue_url":"https://api.github.com/repos/acme/project/issues/18","body":"report","user":{"node_id":"ACTOR_ID"}});
    assert!(decode_comment(wire, &destination).is_err());
}

#[test]
fn captured_scope_requires_complete_bounded_pr_identity() {
    let issue = comment_destination("https://github.com/acme/project/issues/17").unwrap();
    let pull = comment_destination("https://github.com/acme/project/pull/17").unwrap();
    let scope = json!({"baseRepository":"acme/project","baseBranch":"main","baseRevision":"a".repeat(40),"headRepository":"contributor/fork","headBranch":"topic","headRevision":"b".repeat(40)});
    validate_comment_review_scope(&issue, &json!({})).unwrap();
    validate_comment_review_scope(&pull, &scope).unwrap();
    assert!(validate_comment_review_scope(&issue, &scope).is_err());
    assert!(validate_comment_review_scope(&pull, &json!({})).is_err());
    for (field, value) in [
        ("headRevision", "invalid"),
        ("headBranch", "-unsafe"),
        ("baseRepository", "other/project"),
    ] {
        let mut invalid = scope.clone();
        invalid[field] = json!(value);
        assert!(validate_comment_review_scope(&pull, &invalid).is_err());
    }
    let mut partial = scope;
    partial.as_object_mut().unwrap().remove("headRevision");
    assert!(validate_comment_review_scope(&pull, &partial).is_err());
}

#[test]
fn case_variant_comment_destination_accepts_only_the_same_canonical_discussion() {
    let destination = comment_destination("https://github.com/ACME/PROJECT/pull/17").unwrap();
    let wire = json!({"id":23,"html_url":"https://github.com/Acme/Project/pull/17#issuecomment-23","issue_url":"https://api.github.com/repos/Acme/Project/issues/17","body":"report","user":{"node_id":"ACTOR_ID"}});
    let comment = decode_comment(wire.clone(), &destination).unwrap();
    assert_eq!(
        comment.url,
        "https://github.com/Acme/Project/pull/17#issuecomment-23"
    );
    assert_eq!(comment.discussion_url, destination.url);
    for url in [
        "https://api.github.com/repos/Acme/Other/issues/17",
        "https://api.github.com/repos/Acme/Project/issues/18",
        "https://api.github.com/repos/Acme/Project/pull/17",
        "https://api.github.com/repos/Acme/Project/issues/17?redirect=1",
    ] {
        let mut foreign = wire.clone();
        foreign["issue_url"] = json!(url);
        assert!(decode_comment(foreign, &destination).is_err());
    }
}

#[tokio::test]
async fn review_scope_repository_casing_does_not_relax_branch_or_revision_identity() {
    let root = tempfile::tempdir().unwrap();
    let response = json!({"base":{"repo":{"full_name":"Acme/Project"},"ref":"main","sha":"a".repeat(40)},"head":{"repo":{"full_name":"Contributor/Fork"},"ref":"topic","sha":"b".repeat(40)}});
    let script = format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", response);
    write_executable(&root.path().join("gh"), script, 0o700).unwrap();
    let authority = authority(root.path(), "gh");
    let destination = comment_destination("https://github.com/acme/project/pull/17").unwrap();
    let scope = json!({"baseRepository":"acme/project","baseBranch":"main","baseRevision":"a".repeat(40),"headRepository":"contributor/fork","headBranch":"topic","headRevision":"b".repeat(40)});
    authority
        .verify_review_scope(
            &destination,
            &scope,
            SourceDeliveryCredential::token("token"),
        )
        .await
        .unwrap();
    for (field, value) in [
        ("baseBranch", "MAIN".to_owned()),
        ("headRevision", "c".repeat(40)),
    ] {
        let mut different = scope.clone();
        different[field] = json!(value);
        assert!(
            authority
                .verify_review_scope(
                    &destination,
                    &different,
                    SourceDeliveryCredential::token("token")
                )
                .await
                .is_err()
        );
    }
}

fn response_authority(root: &std::path::Path) -> GhCliDeliveryAuthority {
    write_executable(
        &root.join("gh"),
        "#!/usr/bin/python3\nimport os,pathlib,sys\nassert sys.argv[1] == 'api'\nassert os.environ['GH_TOKEN'] == 'private-canary'\nsys.stdout.write(pathlib.Path(sys.argv[0] + '.response').read_text())\n",
        0o700,
    ).unwrap();
    authority(root, "gh")
}

#[tokio::test]
async fn github_actor_and_exact_comment_reads_require_bounded_authoritative_identity() {
    let root = tempfile::tempdir().unwrap();
    let authority = response_authority(root.path());
    let response = root.path().join("gh.response");
    let token = SourceDeliveryCredential::token("private-canary");
    assert_eq!(authority.comment_credential_field(), "GH_TOKEN");
    std::fs::write(
        &response,
        json!({"data":{"viewer":{"id":"ACTOR_ID"}}}).to_string(),
    )
    .unwrap();
    assert_eq!(authority.actor(token).await.unwrap(), "ACTOR_ID");
    for invalid in [
        json!({"data":{"viewer":{"id":""}}}),
        json!({"data":{"viewer":{"id":"x".repeat(257)}}}),
        json!({"data":{"viewer":{"id":"unsafe\nidentity"}}}),
        json!({"data":{"viewer":{"id":"ACTOR_ID"}},"errors":[{"message":"identity unavailable"}]}),
        json!({}),
    ] {
        std::fs::write(&response, invalid.to_string()).unwrap();
        assert!(matches!(
            authority.actor(token).await,
            Err(SourceDeliveryError::Rejected)
        ));
    }
    let destination = comment_destination("https://github.com/acme/project/issues/17").unwrap();
    let wire = json!({"id":23,"html_url":"https://github.com/acme/project/issues/17#issuecomment-23","issue_url":"https://api.github.com/repos/acme/project/issues/17","body":"report","user":{"node_id":"ACTOR_ID"}});
    std::fs::write(&response, wire.to_string()).unwrap();
    let comment = authority
        .get_comment(&destination, "23", token)
        .await
        .unwrap();
    assert_eq!(comment.body, "report");
    assert_eq!(comment.author, "ACTOR_ID");
    for id in ["", "not-a-number"] {
        assert!(matches!(
            authority.get_comment(&destination, id, token).await,
            Err(SourceDeliveryError::Rejected)
        ));
    }
}

#[tokio::test]
async fn malformed_or_oversized_comment_protocol_never_becomes_confirmation() {
    let root = tempfile::tempdir().unwrap();
    let authority = response_authority(root.path());
    let response = root.path().join("gh.response");
    let destination = comment_destination("https://github.com/acme/project/issues/17").unwrap();
    let token = SourceDeliveryCredential::token("private-canary");
    for invalid in ["not-json".to_owned(), "x".repeat(8 * 1024 * 1024 + 1)] {
        std::fs::write(&response, invalid).unwrap();
        let error = authority
            .get_comment(&destination, "23", token)
            .await
            .unwrap_err();
        assert!(matches!(error, SourceDeliveryError::Api(_)));
        assert!(!error.to_string().contains("private-canary"));
    }
    assert!(matches!(
        authority.list_comments(&destination, 0, token).await,
        Err(SourceDeliveryError::Rejected)
    ));
    for invalid in [
        json!({"unexpected":"object"}),
        json!(vec![Value::Null; 101]),
    ] {
        std::fs::write(&response, invalid.to_string()).unwrap();
        assert!(matches!(
            authority.list_comments(&destination, 1, token).await,
            Err(SourceDeliveryError::Rejected)
        ));
    }
}
