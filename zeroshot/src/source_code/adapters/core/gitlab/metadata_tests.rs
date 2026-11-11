use super::*;

#[tokio::test]
async fn existing_active_review_with_empty_description_acquires_generated_metadata() {
    for description in [None, Some("")] {
        adopt_unmarked_description(description).await;
    }
}

#[tokio::test]
async fn existing_active_review_preserves_human_description_when_adopting_generated_metadata() {
    adopt_unmarked_description(Some("Human report: café.  \n/health returns 200\n")).await;
}

async fn adopt_unmarked_description(description: Option<&str>) {
    let fixture = Fixture::new().await;
    fixture.open_review();
    fixture.state.lock().unwrap().review.as_mut().unwrap()["description"] = json!(description);
    let mut next = SourceReviewRequest {
        include_terminal_reviews: false,
        ..request()
    };
    let observed = fixture
        .authority
        .observe_delivery(
            SourceDeliveryRead {
                include_terminal_reviews: false,
                target: &next.target,
                head_branch: &next.head_branch,
                known_review: None,
                include_review: true,
            },
            credential(),
        )
        .await
        .unwrap()
        .review
        .unwrap();
    assert_eq!(observed.review_id, "17");
    let human = description
        .filter(|body| !body.is_empty())
        .map_or(String::new(), |body| format!("{body}\n\n"));
    for (report, writes) in [("Preserve behavior.", 1), ("Updated generated report.", 2)] {
        next.description = report.to_owned();
        for _ in 0..2 {
            let receipt = fixture
                .authority
                .open_or_update_review(&next, credential())
                .await
                .unwrap();
            assert_eq!(receipt.review_id, observed.review_id);
            assert_eq!(receipt.head_revision, HEAD);
            let review = fixture.state.lock().unwrap().review.clone().unwrap();
            assert_eq!(
                review["description"],
                format!(
                    "{human}{DELIVERY_GENERATED_BODY_START}\n{report}\n{DELIVERY_GENERATED_BODY_END}"
                )
            );
            assert_eq!(fixture.writes("/merge_requests/17"), writes);
            assert_eq!(fixture.writes("/merge_requests"), 0);
        }
    }
}

#[test]
fn slash_literalization_is_idempotent_in_prose_code_and_existing_escapes() {
    let cases = [
        (
            "/health returns 200\n/close",
            "\\/health returns 200\n\\/close",
        ),
        (
            " /unknown_action value\n  /close",
            " \\/unknown_action value\n  \\/close",
        ),
        (
            "\\/close\n&#47;close\n`/close`\n> /close",
            "\\/close\n&#47;close\n`/close`\n> /close",
        ),
        (
            "```sh\n/health\n/close\n```\n/close",
            "```sh\n\\/health\n\\/close\n```\n\\/close",
        ),
        (
            "~~~~\n/close\n~~~\n/health\n~~~~\n/close",
            "~~~~\n\\/close\n~~~\n\\/health\n~~~~\n\\/close",
        ),
        (
            "    /close\n\t/health\n/close",
            "    \\/close\n\t\\/health\n\\/close",
        ),
        ("    ```\n/close", "    ```\n\\/close"),
        ("```invalid`fence\n/close", "```invalid`fence\n\\/close"),
        ("/clo\rse\r\n", "\\/close"),
    ];
    for (input, expected) in cases {
        let actual = comment::literal_body(input);
        assert_eq!(actual, expected, "input {input:?}");
        assert_eq!(comment::literal_body(&actual), actual);
    }
}

#[tokio::test]
async fn titles_are_ready_bounded_and_originals_survive_create_update_and_rediscovery_once() {
    let mut cases = vec![
        ("a".repeat(300), "a".repeat(255), true),
        (
            "界🦀".repeat(150),
            "界🦀".repeat(150).chars().take(255).collect(),
            true,
        ),
        ("a".repeat(255), "a".repeat(255), false),
        ("界".repeat(255), "界".repeat(255), false),
        (format!("Draft: {}", "a".repeat(300)), "a".repeat(255), true),
        (
            format!("[Draft] {}", "界🦀".repeat(150)),
            "界🦀".repeat(150).chars().take(255).collect(),
            true,
        ),
    ];
    for prefix in ["Draft:", "[Draft]", "(Draft)", "dRaFt:"] {
        cases.push((format!("{prefix} Ready title"), "Ready title".into(), false));
        cases.push((prefix.into(), "Zeroshot change".into(), false));
    }
    cases.push((
        " [Draft] Draft: (Draft)  ".into(),
        "Zeroshot change".into(),
        false,
    ));
    for (original, expected, shortened) in cases {
        let fixture = Fixture::new().await;
        let mut next = request();
        next.title = original.clone();
        let subject = fixture.authority.prepare_commit_message(&original);
        assert_eq!(subject, expected);
        assert_eq!(fixture.authority.prepare_commit_message(&subject), subject);
        next.description = "/health returns 200\n/close\n\\/close\n```\n/close\n```".into();
        let receipt = fixture
            .authority
            .open_or_update_review(&next, credential())
            .await
            .unwrap();
        let created = fixture.state.lock().unwrap().review.clone().unwrap();
        assert_eq!(created["title"], expected);
        assert!(!expected.is_empty() && expected.chars().count() <= 255);
        let body = created["description"].as_str().unwrap();
        assert!(body.contains("\\/health returns 200\n\\/close\n\\/close\n```\n\\/close\n```"));
        assert_eq!(
            body.matches("Original generated title:").count(),
            usize::from(shortened)
        );
        if shortened {
            assert_eq!(body.matches(&original).count(), 1);
        }
        fixture
            .authority
            .open_or_update_review(&next, credential())
            .await
            .unwrap();
        // A caller may reuse GitLab's already canonical metadata on its next delivery.
        let canonical = SourceReviewRequest {
            title: created["title"].as_str().unwrap().to_owned(),
            description: body
                .strip_prefix(&format!("{DELIVERY_GENERATED_BODY_START}\n"))
                .unwrap()
                .strip_suffix(&format!("\n{DELIVERY_GENERATED_BODY_END}"))
                .unwrap()
                .to_owned(),
            ..next.clone()
        };
        fixture
            .authority
            .open_or_update_review(&canonical, credential())
            .await
            .unwrap();
        assert_eq!(fixture.writes("/merge_requests"), 1);
        assert_eq!(fixture.writes("/merge_requests/17"), 0);
        let prefix = "Human preface.\n\\/close\n```\n/close\n```\n\n";
        let suffix = "\n\nHuman follow-up.  \n";
        fixture.state.lock().unwrap().review.as_mut().unwrap()["description"] =
            json!(format!("{prefix}{body}{suffix}"));
        next.description.push_str("\nUpdated report.");
        fixture
            .authority
            .open_or_update_review(&next, credential())
            .await
            .unwrap();
        fixture
            .authority
            .open_or_update_review(&next, credential())
            .await
            .unwrap();
        let updated = fixture.state.lock().unwrap().review.clone().unwrap();
        let body = updated["description"].as_str().unwrap();
        assert!(body.starts_with(prefix) && body.ends_with(suffix));
        assert!(body.contains("Updated report."));
        assert_eq!(
            body.matches("Original generated title:").count(),
            usize::from(shortened)
        );
        assert_eq!(fixture.writes("/merge_requests/17"), 1);
        let observed = fixture.observe_review(false).await.unwrap().review.unwrap();
        assert_eq!(observed.review_id, receipt.review_id);
        assert_eq!(observed.head_revision, receipt.head_revision);
    }
}

#[tokio::test]
async fn refresh_keeps_a_draft_applied_by_a_person() {
    let fixture = Fixture::new().await;
    let mut next = request();
    fixture
        .authority
        .open_or_update_review(&next, credential())
        .await
        .unwrap();
    {
        let mut state = fixture.state.lock().unwrap();
        let review = state.review.as_mut().unwrap();
        review["title"] = json!("[Draft] Human title");
        review["draft"] = json!(true);
    }
    next.description = "Updated report.".into();
    fixture
        .authority
        .open_or_update_review(&next, credential())
        .await
        .unwrap();
    let review = fixture.state.lock().unwrap().review.clone().unwrap();
    assert_eq!(review["title"], "[Draft] Human title");
    assert_eq!(review["draft"], true);
    assert!(
        review["description"]
            .as_str()
            .unwrap()
            .contains("Updated report.")
    );
}
