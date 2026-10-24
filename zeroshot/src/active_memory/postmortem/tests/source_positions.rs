use super::*;

#[tokio::test]
async fn repository_admission_rejects_volatile_locations_but_preserves_meaningful_numbers() {
    for (field, text, only_folder, rejected) in [
        (
            "recommendation",
            "Use the guard at src/provider.rs:42 before launch.",
            false,
            true,
        ),
        ("title", "Guard in src/provider.rs:42", false, true),
        (
            "recommendation",
            "Use src/provider.rs:42 before launch.",
            true,
            true,
        ),
        (
            "recommendation",
            "In src/provider.rs preserve error E42 and the localhost:8080 endpoint.",
            false,
            false,
        ),
    ] {
        let fixture = fixture(Default::default(), None, true).await;
        fixture.substantial_exploration();
        let mut proposal = candidate("worker");
        proposal[field] = json!(text);
        if only_folder {
            proposal["files"] = json!([]);
            proposal["folders"] = json!(["src"]);
        }
        fixture.output("repository", json!({"lessons":[proposal]}));
        fixture.drive().await;
        assert_eq!(fixture.lessons().await.is_empty(), rejected);
    }
}

#[tokio::test]
async fn legacy_locations_stay_readable_and_require_adaptation_or_deferral() {
    for (action, recommendation, revision, rejected) in [
        ("keep", "", 1_u64, true),
        (
            "adapt",
            "Use the guard at src/provider.rs:84 before launch.",
            1,
            true,
        ),
        (
            "adapt",
            "Call reject_provider_controls in src/provider.rs before provider launch.",
            2,
            false,
        ),
        ("defer", "", 1, false),
    ] {
        let mut legacy = seed_lesson();
        legacy.recommendation =
            "Call the provider guard at src/provider.rs:42 before launch.".to_owned();
        let fixture = fixture(Default::default(), Some(legacy), true).await;
        assert_eq!(fixture.lessons().await[0].revision, 1);
        let mut output = decision(action);
        if action == "adapt" {
            output["recommendation"] = json!(recommendation);
        }
        output["reason"] = json!(
            "Current src/provider.rs identifies the guard; preserve the procedure using stable identifiers instead of source positions."
        );
        fixture.output("repository_maintenance", json!({"decisions":[output]}));
        fixture.drive().await;
        let lessons = fixture.lessons().await;
        assert_eq!(lessons.len(), 1);
        assert_eq!(lessons[0].revision, revision);
        assert_eq!(fixture.maintenance_rejected().await, rejected);
        if revision == 2 {
            assert_eq!(lessons[0].recommendation, recommendation);
            assert_eq!(
                lessons[0].evidence.node_id,
                "active_memory_repository_maintenance"
            );
        }
    }
}

#[tokio::test]
async fn source_location_checks_do_not_reinterpret_urls_ports_or_image_tags() {
    for (file, recommendation, rejected) in [
        (
            "bin/server",
            "Use http://server:8080 for health checks.",
            false,
        ),
        (
            "bin/server",
            "Connect to server:8080 for health checks.",
            false,
        ),
        (
            "src/provider.rs",
            "Use http://provider.rs:8080 for health checks.",
            false,
        ),
        (
            "src/provider.rs",
            "Connect to provider.rs:8080 for health checks.",
            false,
        ),
        (
            "src/provider.rs",
            "Use image src/provider.rs:8080 for the worker.",
            false,
        ),
        (
            "src/provider.rs",
            "Use ghcr.io/src/provider.rs:8080 as the image.",
            false,
        ),
        (
            "src/provider.rs",
            "Call the guard in ./src/provider.rs:42 before launch.",
            true,
        ),
    ] {
        let fixture = fixture(Default::default(), None, true).await;
        let path = fixture._directory.path().join("workspace").join(file);
        std::fs::create_dir_all(path.parent().unwrap()).assert_value();
        std::fs::write(path, "fn reject_provider_controls() {}\n").assert_value();
        fixture.substantial_exploration();
        let mut proposal = candidate("worker");
        proposal["files"] = json!([file]);
        proposal["references"] =
            json!(["hook:1", "hook:2", "hook:3", format!("repository:{file}")]);
        proposal["recommendation"] = json!(recommendation);
        fixture.output("repository", json!({"lessons":[proposal]}));
        fixture.drive().await;
        assert_eq!(
            fixture.lessons().await.is_empty(),
            rejected,
            "{recommendation}"
        );
    }
}

#[tokio::test]
async fn unrelated_reference_context_does_not_exempt_source_locations() {
    for (title, recommendation) in [
        (
            "Docker integration",
            "Use the guard in src/provider.rs:42 before launch.",
        ),
        (
            "Related guard",
            "See https://example.com src/provider.rs:42 before launch.",
        ),
        (
            "Related guard",
            "Use docker image safely. Inspect src/provider.rs:42 before launch.",
        ),
        (
            "Related guard",
            "Use docker image; inspect src/provider.rs:42 before launch.",
        ),
    ] {
        let fixture = fixture(Default::default(), None, true).await;
        fixture.substantial_exploration();
        let mut proposal = candidate("worker");
        proposal["title"] = json!(title);
        proposal["recommendation"] = json!(recommendation);
        fixture.output("repository", json!({"lessons":[proposal]}));
        fixture.drive().await;
        assert!(fixture.lessons().await.is_empty(), "{recommendation}");
    }
}
