use super::*;

#[test]
fn response_contract_distinguishes_repair_pending_and_failure() {
    assert_eq!(
        parse_response(br#"{"verdict":"accepted"}"#).unwrap(),
        DeliveryValidationDecision::Accepted
    );
    assert_eq!(
        parse_response(br#"{"verdict":"pending"}"#).unwrap(),
        DeliveryValidationDecision::Pending
    );
    assert_eq!(
        parse_response(br#"{"verdict":"repair","reason":"Review the new tree"}"#).unwrap(),
        DeliveryValidationDecision::Repair {
            reason: "Review the new tree".to_owned()
        }
    );
    let refused =
        parse_response(br#"{"verdict":"refused","reason":"Backlog changed"}"#).unwrap_err();
    assert!(matches!(refused, NodeRunnerError::DriverDetail(_)));
    assert!(refused.to_string().contains("Backlog changed"));
}

#[test]
fn malformed_responses_never_become_repair_or_expose_raw_output() {
    for bytes in [
        br#"not-json-private-value"#.as_slice(),
        br#"{"verdict":"unknown"}"#,
        br#"{"verdict":"accepted","unexpected":"private-value"}"#,
        br#"{"verdict":"accepted","reason":"private-value"}"#,
        br#"{"verdict":"repair"}"#,
        br#"{"verdict":"repair","reason":""}"#,
        br#"{"verdict":"repair","reason":"\u001bprivate-value"}"#,
        br#"{"verdict":"pending"} {"verdict":"accepted"}"#,
    ] {
        let error = parse_response(bytes).unwrap_err().to_string();
        assert!(!error.contains("private-value"), "{error}");
    }
    let bytes = serde_json::to_vec(&serde_json::json!({
        "verdict": "repair", "reason": "x".repeat(MAX_REASON_BYTES + 1),
    }))
    .unwrap();
    assert!(parse_response(&bytes).is_err());
}

#[cfg(unix)]
mod subprocess {
    use super::*;
    use openengine_cluster_testkit::fixture::write_executable;
    use std::os::unix::fs::{PermissionsExt as _, symlink};
    use std::time::Duration;
    use tokio::sync::watch;

    fn fixture(body: &str) -> (tempfile::TempDir, TrustedDeliveryValidator) {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("checkout");
        std::fs::create_dir(&workspace).unwrap();
        let path = root.path().join("validator.sh");
        write_executable(&path, format!("#!/bin/sh\n{body}\n"), 0o700).unwrap();
        let validator = TrustedDeliveryValidator::new(path, &workspace).unwrap();
        (root, validator)
    }

    fn request() -> DeliveryValidationRequest<'static> {
        DeliveryValidationRequest {
            stage: DeliveryValidationStage::Publish,
            repository: "the-open-engine/zeroshot",
            target_branch: "main",
            head_branch: "zeroshot/example",
            head_revision: "1111111111111111111111111111111111111111",
            tree: "2222222222222222222222222222222222222222",
            review_base_revision: Some("3333333333333333333333333333333333333333"),
            title: "fix: preserve a completed answer",
            description: "## Summary\n\nPreserve a completed answer.",
            review_id: Some("1249"),
        }
    }

    fn captured_request(root: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(root.join("validator.sh.request")).unwrap()).unwrap()
    }

    fn cancellation() -> (watch::Sender<bool>, DriverCancellation) {
        let (signal, receiver) = watch::channel(false);
        (signal, DriverCancellation::new(receiver))
    }

    async fn run(
        validator: &TrustedDeliveryValidator,
    ) -> Result<DeliveryValidationDecision, NodeRunnerError> {
        let (_signal, cancellation) = cancellation();
        validator.validate(request(), cancellation).await
    }

    #[tokio::test]
    async fn pre_cancelled_validation_never_launches_the_private_executable() {
        let (root, validator) = fixture("printf launched > \"$0.started\"");
        let (signal, cancel) = cancellation();
        signal.send(true).unwrap();
        assert!(matches!(
            validator.validate(request(), cancel).await,
            Err(NodeRunnerError::Cancelled)
        ));
        validator.drain().await.unwrap();
        assert!(!root.path().join("validator.sh.started").exists());
    }

    #[tokio::test]
    async fn missing_interpreter_is_a_definite_launch_failure_without_private_path_output() {
        let (_root, validator) = fixture("exit 0");
        write_executable(
            validator.path(),
            "#!/zeroshot-test-missing-interpreter\n",
            0o700,
        )
        .unwrap();
        let error = run(&validator).await.unwrap_err();
        assert!(matches!(error, NodeRunnerError::DriverDetail(_)));
        assert!(error.to_string().contains("could not be started"));
        assert!(
            !error
                .to_string()
                .contains(validator.path().to_str().unwrap())
        );
        validator.drain().await.unwrap();
    }

    #[tokio::test]
    async fn private_executable_gets_candidate_facts_without_ambient_credentials() {
        let (root, validator) = fixture(
            "/bin/cat > \"$0.request\"\n/usr/bin/env > \"$0.environment\"\nprintf '{\"verdict\":\"accepted\"}'",
        );
        assert_eq!(
            run(&validator).await.unwrap(),
            DeliveryValidationDecision::Accepted
        );
        let message = captured_request(root.path());
        assert_eq!(message["kind"], VALIDATION_KIND);
        assert_eq!(message["stage"], "publish");
        assert_eq!(message["headRevision"], request().head_revision);
        assert_eq!(message["tree"], request().tree);
        assert_eq!(
            message["reviewBaseRevision"],
            request().review_base_revision.unwrap()
        );
        assert_eq!(message["title"], request().title);
        assert_eq!(message["description"], request().description);
        assert_eq!(message["reviewId"], "1249");
        let environment =
            std::fs::read_to_string(root.path().join("validator.sh.environment")).unwrap();
        for line in environment.lines() {
            let key = line.split('=').next().unwrap();
            assert!(
                matches!(key, "LANG" | "PWD" | "SHLVL" | "_"),
                "Unexpected environment key: {key}"
            );
        }
        assert!(!format!("{validator:?}").contains(root.path().to_str().unwrap()));
    }

    #[tokio::test]
    async fn merge_stage_and_nullable_facts_are_transmitted() {
        let (root, validator) =
            fixture("/bin/cat > \"$0.request\"\nprintf '{\"verdict\":\"pending\"}'");
        let (_signal, cancel) = cancellation();
        let mut input = request();
        input.stage = DeliveryValidationStage::Merge;
        input.review_base_revision = None;
        input.review_id = None;
        assert_eq!(
            validator.validate(input, cancel).await.unwrap(),
            DeliveryValidationDecision::Pending
        );
        let message = captured_request(root.path());
        assert_eq!(message["stage"], "merge");
        assert!(message["reviewBaseRevision"].is_null());
        assert!(message["reviewId"].is_null());
    }

    #[test]
    fn executable_must_be_private_and_outside_the_checkout() {
        let (root, validator) = fixture("exit 0");
        let workspace = root.path().join("checkout");
        assert!(TrustedDeliveryValidator::new("relative".into(), &workspace).is_err());
        let inside = workspace.join("validator");
        write_executable(&inside, "#!/bin/sh\nexit 0\n", 0o700).unwrap();
        assert!(TrustedDeliveryValidator::new(inside, &workspace).is_err());
        std::fs::set_permissions(validator.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(TrustedDeliveryValidator::new(validator.path().into(), &workspace).is_err());
        std::fs::set_permissions(validator.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(TrustedDeliveryValidator::new(validator.path().into(), &workspace).is_err());
    }

    #[test]
    fn checkout_aliases_and_final_executable_symlinks_are_rejected() {
        let (root, validator) = fixture("exit 0");
        let workspace = root.path().join("checkout");
        let alias = root.path().join("alias");
        symlink(&workspace, &alias).unwrap();
        let inside = workspace.join("validator");
        write_executable(&inside, "#!/bin/sh\nexit 0\n", 0o700).unwrap();
        assert!(TrustedDeliveryValidator::new(alias.join("validator"), &workspace).is_err());
        let alias = root.path().join("validator-alias");
        symlink(validator.path(), &alias).unwrap();
        assert!(TrustedDeliveryValidator::new(alias, &workspace).is_err());
    }

    #[tokio::test]
    async fn path_security_is_rechecked_before_each_execution() {
        let (root, validator) = fixture("/bin/cat >/dev/null\nprintf '{\"verdict\":\"accepted\"}'");
        assert_eq!(
            run(&validator).await.unwrap(),
            DeliveryValidationDecision::Accepted
        );
        std::fs::remove_file(validator.path()).unwrap();
        let inside = root.path().join("checkout/validator");
        write_executable(&inside, "#!/bin/sh\nexit 0\n", 0o700).unwrap();
        symlink(&inside, validator.path()).unwrap();
        assert!(run(&validator).await.is_err());
    }

    #[tokio::test]
    async fn nonzero_exit_never_leaks_stdout_or_stderr_or_accepts_a_verdict() {
        let (_root, validator) = fixture(
            "/bin/cat >/dev/null\nprintf '{\"verdict\":\"accepted\"}'\nprintf 'private-raw-output' >&2\nexit 7",
        );
        let error = run(&validator).await.unwrap_err().to_string();
        assert!(!error.contains("private-raw-output"), "{error}");
        assert!(error.contains("did not complete successfully"), "{error}");
    }

    #[tokio::test]
    async fn concurrent_pipes_allow_large_requests_and_early_responses() {
        let (_root, validator) = fixture(
            "/bin/dd if=/dev/zero bs=65536 count=4 2>/dev/null | /usr/bin/tr '\\000' ' '\n/bin/cat >/dev/null\nprintf '{\"verdict\":\"accepted\"}'",
        );
        let description = "x".repeat(512 * 1024);
        let mut input = request();
        input.description = &description;
        let (_signal, cancel) = cancellation();
        let outcome =
            tokio::time::timeout(Duration::from_secs(10), validator.validate(input, cancel))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(outcome, DeliveryValidationDecision::Accepted);
    }

    #[tokio::test]
    async fn oversized_requests_fail_before_launch_and_responses_stop_the_child() {
        let (root, validator) = fixture(
            "printf started > \"$0.started\"\n/bin/cat >/dev/null\nprintf '{\"verdict\":\"accepted\"}'",
        );
        let description = "x".repeat(MAX_VALIDATION_BYTES + 1);
        let mut input = request();
        input.description = &description;
        let (_signal, cancel) = cancellation();
        assert!(
            validator
                .validate(input, cancel)
                .await
                .unwrap_err()
                .to_string()
                .contains("byte bound")
        );
        assert!(!root.path().join("validator.sh.started").exists());
        let (_root, validator) =
            fixture("/bin/dd if=/dev/zero bs=65536 count=32 2>/dev/null\n/bin/sleep 60");
        let error = tokio::time::timeout(Duration::from_secs(10), run(&validator))
            .await
            .unwrap()
            .unwrap_err()
            .to_string();
        assert!(!error.contains('\0'));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancellation_reaps_the_helper_and_its_descendant() {
        let (root, validator) = fixture(
            "/bin/cat >/dev/null\n/bin/sleep 60 &\nprintf '%s' \"$!\" > \"$0.child\"\nwait",
        );
        let (signal, cancel) = cancellation();
        let running = tokio::spawn(async move { validator.validate(request(), cancel).await });
        let marker = root.path().join("validator.sh.child");
        tokio::time::timeout(Duration::from_secs(10), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let pid: i32 = std::fs::read_to_string(marker).unwrap().parse().unwrap();
        signal.send(true).unwrap();
        let outcome = tokio::time::timeout(Duration::from_secs(10), running)
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(outcome, Err(NodeRunnerError::Cancelled)),
            "{outcome:?}"
        );
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "Helper descendant is still live"
        );
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn successful_exit_reaps_descendants_holding_inherited_pipes() {
        let (root, validator) = fixture(
            "/bin/cat >/dev/null\n/bin/sleep 60 &\nprintf '%s' \"$!\" > \"$0.child\"\nprintf '{\"verdict\":\"accepted\"}'",
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), run(&validator))
                .await
                .unwrap()
                .unwrap(),
            DeliveryValidationDecision::Accepted
        );
        let pid: i32 = std::fs::read_to_string(root.path().join("validator.sh.child"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "Helper descendant is still live"
        );
    }
}
