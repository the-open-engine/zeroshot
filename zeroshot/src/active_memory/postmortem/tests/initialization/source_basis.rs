use super::*;

fn exported_basis(case: &Case, inventory: &Value) -> Value {
    let mut basis = None;
    for task in inventory["runs"][0]["tasks"].as_array().assert_value() {
        let payload = read(&case.session.join(task["path"].as_str().assert_value()));
        let current = payload["context"]["source_basis"].clone();
        if let Some(expected) = &basis {
            assert_eq!(expected, &current);
        } else {
            basis = Some(current);
        }
    }
    basis.assert_value()
}

fn assert_basis(lesson: &Value, expected: Option<&str>) {
    let references = lesson["evidence"]["references"].as_array().assert_value();
    let source = references
        .iter()
        .filter_map(Value::as_str)
        .filter(|reference| reference.starts_with("source:"))
        .collect::<Vec<_>>();
    assert_eq!(source, expected.into_iter().collect::<Vec<_>>());
}

fn assert_staged_basis(case: &Case, expected: Option<&str>) {
    let review = read(&case.session.join("review.json"));
    let lessons = review["snapshot"]["lessons"].as_array().assert_value();
    assert_eq!(lessons.len(), 2);
    for lesson in lessons {
        assert_basis(lesson, expected);
        if let Some(record) = case
            .store()
            .record(lesson["id"].as_str().assert_value())
            .assert_value()
        {
            assert_basis(
                &serde_json::to_value(record.lesson.assert_value()).assert_value(),
                expected,
            );
        }
    }
}

fn restore(case: &Case, basis: &str) {
    git(
        &case.workspace(),
        &[
            "checkout",
            "--quiet",
            "--detach",
            basis.strip_prefix("source:").assert_value(),
        ],
    );
}

async fn rejects_changed_basis(case: &Case) {
    let refused = initialize::init(case.state.clone(), &case.session, Default::default())
        .await
        .unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("source changed since task export")
    );
    assert_eq!(
        case.store()
            .snapshot(&case.fixture.query)
            .assert_value()
            .generation,
        0
    );
}

#[tokio::test]
async fn initialization_binds_discovery_responses_to_the_exported_clean_checkout() {
    let case = case().await;
    let inventory = case.advance().await;
    let basis = exported_basis(&case, &inventory);
    let original = basis.as_str().assert_value();
    case.select(&inventory);
    case.discoveries(&inventory, true);
    std::fs::write(
        case.workspace().join("src/provider.rs"),
        "fn reject_provider_controls() { /* source changed after discovery */ }\n",
    )
    .assert_value();
    let changed = fixture_source(&case.workspace());
    assert_ne!(original, changed);
    rejects_changed_basis(&case).await;
    assert!(!case.session.join("review.json").exists());

    // Restoring the actual exported source makes the original digest-bound response usable.
    restore(&case, original);
    let review = case.advance().await;
    assert_eq!(review["phase"], "maintenance");
    assert_staged_basis(&case, Some(original));
    case.maintenance(&review, "keep");
    assert_eq!(case.advance().await["published_mutations"], 2);
    assert_staged_basis(&case, Some(original));
}

#[tokio::test]
async fn initialization_keeps_unknown_discovery_provenance_after_the_checkout_becomes_clean() {
    let case = case().await;
    std::fs::write(
        case.workspace().join("src/provider.rs"),
        "fn reject_provider_controls() { /* inspected while uncommitted */ }\n",
    )
    .assert_value();
    let inventory = case.advance().await;
    assert!(exported_basis(&case, &inventory).is_null());
    case.select(&inventory);
    case.discoveries(&inventory, true);
    let clean = fixture_source(&case.workspace());
    assert!(clean.starts_with("source:"));
    let review = case.advance().await;
    assert_eq!(review["phase"], "maintenance");
    assert_staged_basis(&case, None);
    case.maintenance(&review, "keep");
    assert_eq!(case.advance().await["published_mutations"], 2);
    assert_staged_basis(&case, None);
}

#[tokio::test]
async fn initialization_binds_maintenance_responses_even_when_checkout_files_are_unchanged() {
    let case = case().await;
    case.stage().await;
    let review = case.advance().await;
    assert_eq!(review["phase"], "maintenance");
    let task = &review["maintenance_tasks"][0]["task"];
    let payload = read(&case.session.join(task["path"].as_str().assert_value()));
    let basis = payload["context"]["source_basis"].as_str().assert_value();
    case.reconcile(&review, |lesson| {
        let mut decision = maintenance(lesson, "adapt");
        decision["title"] = json!("Check the complete provider guard");
        decision["recommendation"] =
            json!("Inspect cached OAuth credentials and every provider control before retrying.");
        if lesson["scope"]["kind"] == "repository" {
            decision["files"] = json!(["src/provider.rs"]);
        }
        decision
    });
    git(
        &case.workspace(),
        &[
            "-c",
            "user.name=Memory fixture",
            "-c",
            "user.email=memory@example.invalid",
            "commit",
            "--allow-empty",
            "--quiet",
            "--no-gpg-sign",
            "-m",
            "Source metadata only",
        ],
    );
    rejects_changed_basis(&case).await;
    restore(&case, basis);
    assert_eq!(case.advance().await["published_mutations"], 2);
    let staged = read(&case.session.join("review.json"));
    for lesson in staged["snapshot"]["lessons"].as_array().assert_value() {
        let published = case
            .store()
            .record(lesson["id"].as_str().assert_value())
            .assert_value()
            .assert_value()
            .lesson
            .assert_value();
        assert_eq!(published.title, "Check the complete provider guard");
        assert_basis(&serde_json::to_value(published).assert_value(), Some(basis));
    }
}
