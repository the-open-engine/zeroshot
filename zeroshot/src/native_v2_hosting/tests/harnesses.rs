use super::*;

#[tokio::test]
async fn forced_contributed_harness_requests_fail_before_connections_or_allocation() {
    for override_lane in [false, true] {
        let root = TestDirectory::new("hosting-contributed-refusal");
        let mut config = hosting_config(root.path().to_owned());
        config.git_program = root.path().join("git-must-not-run");
        let factory = ProductionTargetControllerFactory::new(config);
        let catalog = factory.runtime_harnesses().assert_value();
        assert!(
            catalog
                .lanes
                .iter()
                .all(|lane| lane.harness_name() != "opencode")
        );
        let controller = factory.create_controller().await.assert_value();
        let mut sourceful = submission(
            runtime(BTreeSet::new()),
            &"a".repeat(40),
            "forced-contribution",
        );
        let opencode =
            openengine_cluster_protocol::RuntimeLane::new("opencode", "openai").assert_value();
        if override_lane {
            let Some(crate::native_v2_contract::NodeRuntimeBinding::Agent { lane, .. }) =
                sourceful.runtime.nodes_mut().values_mut().next()
            else {
                panic!("fixture binds one agent");
            };
            *lane = Some(opencode);
        } else {
            sourceful.runtime.harness = opencode.harness;
            sourceful.runtime.provider = opencode.provider;
        }
        let request = crate::native_v2_target_authority::TargetRunRequest {
            run_id: RunId::new("018f5e78-7f95-7c22-8d98-3f15af20c991"),
            submission: sourceful,
            connections: BTreeMap::new(),
            connection_resolver: None,
            git_credentials: None,
            github_token: None,
        };
        let error = factory.submit(&controller, request).await.assert_error();
        assert_eq!(error.kind(), TargetAuthorityErrorKind::Invalid);
        assert!(error.message().contains("opencode") && error.message().contains("not enabled"));
        assert_eq!(
            fs::read_dir(root.path().join("runs"))
                .assert_value()
                .count(),
            0
        );
    }
}
