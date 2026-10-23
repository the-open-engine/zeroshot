use super::*;

pub(super) async fn source_case(with_remote: bool) -> Case {
    let mut case = case().await;
    git(
        &case.workspace(),
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    );
    git(
        &case.workspace(),
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/example/memory-fixture.git",
        ],
    );
    let (_, source) =
        crate::native_v2_local::local_resolved_source(&case.workspace(), Path::new("git"))
            .assert_value();
    case.stored.admitted.source = Some(source.assert_value());
    if !with_remote {
        case.stored.admitted.source.as_mut().assert_value().remote = None;
    }
    write_history(&case.state, &case.workspace(), &case.stored);
    case
}

#[tokio::test]
async fn source_only_history_matches_current_repository_identity_attached_or_detached() {
    for detached in [false, true] {
        let case = source_case(false).await;
        if detached {
            git(&case.workspace(), &["checkout", "--quiet", "--detach"]);
        }
        let inventory = case.advance().await;
        assert_eq!(inventory["runs"].as_array().assert_value().len(), 1);
        assert!(inventory["excluded"].as_array().assert_value().is_empty());
        let historical =
            crate::active_memory::runtime::repository_key(&case.stored.admitted, &case.workspace());
        assert!(
            inventory["runs"][0]["repository_aliases"]
                .as_array()
                .assert_value()
                .contains(&json!(historical))
        );
        assert_ne!(inventory["runs"][0]["repository"], json!(historical));
    }
}

#[tokio::test]
async fn current_repository_proof_rejects_replacement_with_or_without_historical_remote() {
    for with_remote in [false, true] {
        let case = source_case(with_remote).await;
        git(
            &case.workspace(),
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/other/memory-fixture.git",
            ],
        );
        assert_identity_excluded(&case).await;
    }
}

#[tokio::test]
async fn historical_display_alias_cannot_prove_a_source_only_repository_identity() {
    let mut case = source_case(false).await;
    case.stored
        .admitted
        .source
        .as_mut()
        .assert_value()
        .repository =
        openengine_cluster_protocol::SourceRepositoryId::new("other/memory-fixture").assert_value();
    write_history(&case.state, &case.workspace(), &case.stored);
    assert_identity_excluded(&case).await;
}

async fn assert_identity_excluded(case: &Case) {
    let inventory = case.prepare().await;
    assert!(inventory["runs"].as_array().assert_value().is_empty());
    assert!(
        inventory["excluded"][0]["reason"]
            .as_str()
            .assert_value()
            .contains("different repository identity")
    );
}
