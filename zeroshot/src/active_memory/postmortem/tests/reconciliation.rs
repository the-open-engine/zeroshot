use super::*;

#[tokio::test]
async fn maintenance_batches_cover_all_supplied_lessons_and_reject_an_incomplete_last_batch() {
    for omitted in [None, Some("supplied-0599")] {
        let lessons = (0..600)
            .map(|index| {
                let mut draft = seed_lesson();
                draft.id = format!("supplied-{index:04}");
                draft
            })
            .collect();
        let fixture = fixture_with_lessons(Default::default(), lessons, true).await;
        *fixture.driver.maintenance_action.lock().assert_value() = Some("retire");
        *fixture.driver.maintenance_omit.lock().assert_value() = omitted.map(str::to_owned);
        fixture.drive().await;
        let retained = fixture.lessons().await;
        assert_eq!(fixture.maintenance_rejected().await, omitted.is_some());
        if omitted.is_some() {
            assert_eq!(
                retained.len(),
                600,
                "a failed later batch must not publish earlier retirements"
            );
            assert!(retained.iter().all(|lesson| lesson.revision == 1));
        } else {
            assert!(retained.is_empty(), "every supplied lesson must be retired");
        }
        let all_inputs = fixture.driver.maintenance_inputs.lock().assert_value();
        let inputs = all_inputs
            .iter()
            .filter(|input| input["role"] == "active_memory_repository_maintenance")
            .collect::<Vec<_>>();
        assert!(
            inputs.len() > 1,
            "the manifest must exceed one context budget"
        );
        let mut seen = std::collections::BTreeSet::new();
        for (index, input) in inputs.iter().enumerate() {
            assert!(context_budget::encoded_size(input) <= context_budget::MAX_INPUT_BYTES);
            assert_eq!(
                input["maintenance_batch"],
                json!({"index":index + 1,"total":inputs.len()})
            );
            assert_eq!(input["supplied_manifest_total"], 600);
            for id in input["supplied_lessons"].as_object().assert_value().keys() {
                assert!(
                    seen.insert(id.clone()),
                    "a lesson was reviewed more than once"
                );
            }
        }
        assert_eq!(seen.len(), 600);
        assert!(seen.contains("supplied-0599"));
        assert_eq!(
            fixture.driver.globals.load(Ordering::SeqCst),
            inputs.len() + 3
        );
        assert_eq!(
            fixture.driver.closed.load(Ordering::SeqCst),
            5,
            "maintenance batches reuse their specialist session"
        );
    }
}

#[tokio::test]
async fn omitted_duplicate_or_foreign_maintenance_decisions_cannot_claim_completion() {
    let retire = decision("retire");
    let mut foreign = retire.clone();
    foreign["id"] = json!("not-supplied-in-this-batch");
    for decisions in [
        vec![],
        vec![retire.clone(), retire.clone()],
        vec![retire.clone(), foreign],
    ] {
        let fixture = fixture(Default::default(), Some(seed_lesson()), true).await;
        fixture.output("repository_maintenance", json!({"decisions":decisions}));
        fixture.drive().await;
        assert!(fixture.maintenance_rejected().await);
        let lessons = fixture.lessons().await;
        assert_eq!(lessons.len(), 1);
        assert_eq!(
            lessons[0].revision, 1,
            "failed reconciliation must not publish a partial result"
        );
    }
}

#[tokio::test]
async fn discovery_publishes_all_qualifying_findings_without_a_count_quota() {
    let fixture = fixture(Default::default(), None, true).await;
    fixture.substantial_exploration();
    let lessons = (0..129).map(|index| {
        let mut finding = candidate("worker");
        finding["title"] = json!(format!("Provider control interaction {index}"));
        finding["recommendation"] = json!(format!("For provider control interaction {index}, reject cached OAuth credentials before constructing the Bedrock environment."));
        finding
    }).collect::<Vec<_>>();
    fixture.output("repository", json!({"lessons":lessons}));
    fixture.drive().await;
    let stored = fixture.lessons().await;
    assert_eq!(stored.len(), 129);
    assert!(
        stored
            .iter()
            .any(|lesson| lesson.title == "Provider control interaction 128")
    );
}
