use super::*;

async fn mixed_lessons() -> Vec<LessonDraft> {
    let template = fixture(Default::default(), None, true).await;
    let repository = seed_lesson();
    let mut graph = seed_lesson();
    graph.id = "existing-graph-lesson".to_owned();
    graph.scope = LessonScope::Graph {
        graph_hash: template.query.graph_hash.clone().assert_value(),
        node_id: "worker".to_owned(),
        repository: Some("example/repo".to_owned()),
    };
    vec![repository, graph]
}

#[tokio::test]
async fn maintenance_specialists_receive_disjoint_scopes_and_record_their_own_provenance() {
    let fixture = fixture_with_lessons(Default::default(), mixed_lessons().await, true).await;
    *fixture.driver.maintenance_action.lock().assert_value() = Some("adapt");
    fixture.drive().await;
    assert!(!fixture.maintenance_rejected().await);
    let lessons = fixture.lessons().await;
    assert_eq!(lessons.len(), 2);
    for lesson in lessons {
        assert_eq!(lesson.revision, 2);
        let expected = match lesson.scope {
            LessonScope::Graph { repository, .. } => {
                assert_eq!(repository.as_deref(), Some("example/repo"));
                "active_memory_graph_maintenance"
            }
            LessonScope::Repository { .. } => "active_memory_repository_maintenance",
        };
        assert_eq!(lesson.evidence.node_id, expected);
    }
    let inputs = fixture.driver.maintenance_inputs.lock().assert_value();
    assert_eq!(inputs.len(), 2);
    for input in inputs.iter() {
        let supplied = input["supplied_lessons"].as_object().assert_value();
        assert_eq!(supplied.len(), 1);
        assert_eq!(input["supplied_manifest_total"], 1);
        assert_eq!(input["known_lesson_count"], 1);
        let (id, lesson) = supplied.iter().next().assert_value();
        if input["role"] == "active_memory_graph_maintenance" {
            assert_eq!(id, "existing-graph-lesson");
            assert_eq!(lesson["scope"]["kind"], "graph");
        } else {
            assert_eq!(input["role"], "active_memory_repository_maintenance");
            assert_eq!(id, "existing-provider-lesson");
            assert_eq!(lesson["scope"]["kind"], "repository");
        }
    }
}

#[tokio::test]
async fn a_maintenance_specialist_cannot_retire_the_other_scopes_lesson() {
    for (role, foreign_id) in [
        ("graph_maintenance", "existing-provider-lesson"),
        ("repository_maintenance", "existing-graph-lesson"),
    ] {
        let fixture = fixture_with_lessons(Default::default(), mixed_lessons().await, true).await;
        *fixture.driver.maintenance_action.lock().assert_value() = Some("adapt");
        let mut foreign = decision("retire");
        foreign["id"] = json!(foreign_id);
        fixture.output(role, json!({"decisions":[foreign]}));
        fixture.drive().await;
        assert!(fixture.maintenance_rejected().await);
        let lessons = fixture.lessons().await;
        assert_eq!(lessons.len(), 2, "a foreign ID cannot authorize deletion");
        for lesson in lessons {
            assert_eq!(
                lesson.revision,
                if lesson.id == foreign_id { 2 } else { 1 },
                "the other specialist must remain able to adapt its own lesson"
            );
        }
    }
}

#[tokio::test]
async fn each_scope_opt_out_disables_its_maintenance_and_discovery_without_touching_its_lessons() {
    for (repository, graph, expected_roles, expected_retained) in [
        (
            false,
            true,
            vec!["active_memory_graph_maintenance", "active_memory_graph"],
            vec!["existing-provider-lesson"],
        ),
        (
            true,
            false,
            vec![
                "active_memory_repository_maintenance",
                "active_memory_repository",
            ],
            vec!["existing-graph-lesson"],
        ),
        (
            false,
            false,
            vec![],
            vec!["existing-graph-lesson", "existing-provider-lesson"],
        ),
    ] {
        let settings = ActiveMemoryConfig {
            repository,
            graph,
            ..Default::default()
        };
        let fixture = fixture_with_lessons(settings, mixed_lessons().await, true).await;
        *fixture.driver.maintenance_action.lock().assert_value() = Some("retire");
        fixture.drive().await;
        assert!(!fixture.maintenance_rejected().await);
        let retained = fixture
            .lessons()
            .await
            .into_iter()
            .map(|lesson| lesson.id)
            .collect::<Vec<_>>();
        assert_eq!(retained, expected_retained);
        let slots = fixture.driver.slots.lock().assert_value();
        let executed = slots
            .iter()
            .filter(|(name, _)| name.starts_with("active_memory_"))
            .map(|(name, _)| name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(executed, expected_roles.into_iter().collect());
    }
}
