use super::*;

// Only forge feedback is real here. Existing local Git and merge fixtures keep all effects private.
fn feedback_authority(repo: &TempRepo, permission: &str) -> Arc<FakeGitHub> {
    let program = write_executable(
        repo.root.path(),
        "gh-feedback",
        r#"#!/bin/sh
set -eu
case "$2" in
repos/acme/project/pulls/17) cat "$0.review" ;;
'repos/acme/project/issues/17/comments?per_page=100')
  count=0
  if [ -f "$0.reads" ]; then count=$(cat "$0.reads"); fi
  count=$((count + 1))
  printf '%s' "$count" > "$0.reads"
  version=one
  if [ "$count" -ge 2 ]; then version=two; fi
  printf '[[{"id":41,"updated_at":"%s","user":{"id":7,"login":"reviewer"},"author_association":"MEMBER","body":"Check case %s"}]]\n' "$version" "$version" ;;
'repos/acme/project/pulls/17/reviews?per_page=100'|'repos/acme/project/pulls/17/comments?per_page=100') printf '[[]]\n' ;;
repos/acme/project/collaborators/reviewer/permission)
  if [ -f "$0.error" ]; then cat "$0.error" >&2; exit 1; fi
  cat "$0.permission" ;;
*) exit 19 ;;
esac
"#,
    );
    let response = if permission == "missing" {
        json!({"user":{"id":7,"login":"reviewer"}})
    } else {
        json!({"permission":permission,"user":{"id":7,"login":"reviewer"}})
    };
    fs::write(
        format!("{}.permission", program.display()),
        response.to_string(),
    )
    .unwrap();
    let forge = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        gh_program: program.clone(),
        api_deadline: Duration::from_secs(30),
        ..GhCliAuthorityConfig::hosted(repo.root.path().to_owned())
    });
    let mut authority = FakeGitHub::new(repo.remote.clone(), Script::RegistrationRace);
    authority.feedback_authority = Some((forge, program));
    Arc::new(authority)
}

#[tokio::test]
async fn new_and_edited_outsider_feedback_never_dispatches_a_repair() {
    for permission in ["none", "read", "404"] {
        let repo = TempRepo::delivery();
        let authority = feedback_authority(&repo, permission);
        if permission == "404" {
            let program = &authority.feedback_authority.as_ref().unwrap().1;
            fs::write(
                format!("{}.error", program.display()),
                "gh: Not Found (HTTP 404)\n",
            )
            .unwrap();
        }
        let (terminal, lane, run_id, ledger) =
            drive_repair_loop(&repo, authority.clone(), DeliveryMode::MergeV3).await;
        assert!(matches!(terminal, TerminalResult::Succeeded { .. }));
        assert_eq!(lane.repairs.load(Ordering::SeqCst), 0);
        assert_eq!(authority.merge_requests.load(Ordering::SeqCst), 2);
        assert!(
            authority.feedback_reads.load(Ordering::SeqCst) >= 2,
            "must inspect both new and edited feedback"
        );
        let stored = ledger.get(&run_id).await.unwrap().unwrap();
        assert!(
            stored
                .snapshot
                .executions
                .values()
                .all(|execution| execution.reference.node.as_str() == "deliver")
        );
    }
}

#[tokio::test]
async fn authorized_new_and_edited_feedback_each_dispatch_one_repair_then_deduplicate() {
    let repo = TempRepo::delivery();
    let authority = feedback_authority(&repo, "write");
    let (terminal, lane, run_id, ledger) =
        drive_repair_loop(&repo, authority.clone(), DeliveryMode::MergeV3).await;
    assert!(matches!(terminal, TerminalResult::Succeeded { .. }));
    assert_eq!(lane.repairs.load(Ordering::SeqCst), 2);
    assert_eq!(authority.merge_requests.load(Ordering::SeqCst), 2);
    let stored = ledger.get(&run_id).await.unwrap().unwrap();
    let repairs = stored
        .snapshot
        .executions
        .values()
        .filter(|execution| execution.reference.node.as_str() == "repair")
        .count();
    assert_eq!(repairs, 2);
}

#[tokio::test]
async fn incomplete_permission_reads_never_merge_or_dispatch_a_repair() {
    for failure in ["missing", "401", "403", "429", "transport"] {
        let repo = TempRepo::delivery();
        let authority = feedback_authority(&repo, "missing");
        let program = &authority.feedback_authority.as_ref().unwrap().1;
        let diagnostic = match failure {
            "401" => Some("gh: Bad credentials (HTTP 401)\n"),
            "403" => Some("gh: Resource not accessible (HTTP 403)\n"),
            "429" => Some("gh: Too Many Requests (HTTP 429)\n"),
            "transport" => Some("error connecting to api.github.com\n"),
            _ => None,
        };
        if let Some(diagnostic) = diagnostic {
            fs::write(format!("{}.error", program.display()), diagnostic).unwrap();
        }
        let (terminal, lane, run_id, ledger) =
            drive_repair_loop(&repo, authority.clone(), DeliveryMode::MergeV3).await;
        assert!(
            matches!(terminal, TerminalResult::Failed { .. }),
            "{failure}: {terminal:?}"
        );
        assert_eq!(lane.repairs.load(Ordering::SeqCst), 0);
        assert_eq!(authority.merge_requests.load(Ordering::SeqCst), 0);
        let stored = ledger.get(&run_id).await.unwrap().unwrap();
        assert!(
            stored
                .snapshot
                .executions
                .values()
                .all(|execution| execution.reference.node.as_str() == "deliver")
        );
    }
}
