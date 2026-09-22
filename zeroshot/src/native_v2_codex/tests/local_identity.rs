use super::*;

#[test]
fn local_and_managed_openai_auth_preserve_explicit_codex_keys() {
    for has_local_user in [true, false] {
        let mut values = BTreeMap::from([
            (
                "OPENAI_API_KEY".to_owned(),
                "custom-provider-key".to_owned(),
            ),
            ("CODEX_API_KEY".to_owned(), "native-provider-key".to_owned()),
        ]);
        configure_provider_auth(&mut values, CodexProvider::OpenAi, has_local_user).assert_value();
        assert_eq!(
            values.get("CODEX_API_KEY").map(String::as_str),
            Some("native-provider-key")
        );
        assert_eq!(
            values.get("OPENAI_API_KEY").map(String::as_str),
            has_local_user.then_some("custom-provider-key")
        );
    }
}

#[test]
fn declared_codex_auth_family_suppresses_every_ambient_alias() {
    let directory = TestDirectory::new("codex-auth-precedence");
    let mut configuration = scripted_adapter(&directory, CodexProvider::OpenAi)
        .config
        .clone();
    configuration.local_user = Some(NativeV2CodexUser {
        home: directory.child("home"),
        codex_home: directory.child("codex-home"),
    });
    let mut adapter = NativeV2CodexAdapter::new_for_test(configuration);

    for (declared_name, ambient_name) in [
        ("OPENAI_API_KEY", "CODEX_API_KEY"),
        ("CODEX_API_KEY", "OPENAI_API_KEY"),
    ] {
        adapter.local_environment = BTreeMap::from([
            (declared_name.to_owned(), "ambient-shadow".to_owned()),
            (ambient_name.to_owned(), "ambient-conflict".to_owned()),
        ]);
        let binding = binding(SessionScope::Execution, &[declared_name]);
        let environment = ResolvedEnvironment::exact(
            &binding,
            BTreeMap::from([(environment_name(declared_name), "declared-key".to_owned())]),
        )
        .assert_value();
        let values = adapter
            .provider_environment(&environment, &directory.child("runtime"))
            .assert_value();
        assert_eq!(values["CODEX_API_KEY"], "declared-key");
        assert!(
            values
                .get("OPENAI_API_KEY")
                .is_none_or(|value| value == "declared-key")
        );
        assert!(!values.values().any(|value| value == "ambient-conflict"));
        assert!(!values.values().any(|value| value == "ambient-shadow"));
    }
}

#[test]
fn local_codex_user_reuses_native_homes_without_an_openai_api_key() {
    let directory = TestDirectory::new("codex-local-user");
    let runtime_home = directory.child("runtime");
    let home = directory.child("home");
    let codex_home = directory.child("codex-home");
    let adapter = NativeV2CodexAdapter::new_for_test(NativeV2CodexConfig {
        provider: CodexProvider::OpenAi,
        executable: PathBuf::from("codex"),
        workspace: directory.path().to_owned(),
        runtime_home: runtime_home.clone(),
        local_user: Some(NativeV2CodexUser {
            home: home.clone(),
            codex_home: codex_home.clone(),
        }),
        native_environment: Default::default(),
        search_path: "/usr/bin:/bin".to_owned(),
        process_pool: HostedProcessPool::new(10_002, 10_002, 20_000, 20_000).assert_value(),
    });
    let binding = binding(SessionScope::Execution, &[]);
    let environment = ResolvedEnvironment::exact(&binding, BTreeMap::new()).assert_value();
    let values = adapter
        .provider_environment(&environment, &runtime_home)
        .assert_value();

    assert_eq!(values.get("HOME").map(String::as_str), home.to_str());
    assert_eq!(
        values.get("CODEX_HOME").map(String::as_str),
        codex_home.to_str()
    );
    assert!(!values.contains_key("CODEX_API_KEY"));

    let isolated = NativeV2CodexAdapter::new(adapter.config.clone());
    assert_eq!(
        isolated.provider_environment(&environment, &runtime_home),
        Err(NodeRunnerError::DriverDetail(
            "Codex provider credentials are missing or conflict with reserved credentials"
                .to_owned()
        ))
    );
}

#[tokio::test]
async fn local_codex_turns_preserve_litellm_configuration_and_credentials() {
    let directory = TestDirectory::new("codex-local-config");
    let home = directory.child("home");
    let codex_home = directory.child("custom-codex-home");
    fs::create_dir_all(&home).assert_value();
    fs::create_dir_all(&codex_home).assert_value();
    let config = concat!(
        "model_provider = \"litellm\"\n",
        "web_search = \"live\"\n",
        "model_reasoning_effort = \"low\"\n",
        "[sandbox_workspace_write]\n",
        "network_access = false\n",
        "[model_providers.litellm]\n",
        "name = \"LiteLLM\"\n",
        "base_url = \"http://127.0.0.1:4000/v1\"\n",
        "env_key = \"OPENAI_API_KEY\"\n",
        "wire_api = \"responses\"\n",
    );
    let config_path = codex_home.join("config.toml");
    fs::write(&config_path, config).assert_value();
    let script = SCRIPT.replace(
        "schema_path=\n",
        "/usr/bin/cat \"$CODEX_HOME/config.toml\" >> \"$CONFIG_CAPTURE\"\nschema_path=\n",
    );
    let adapter = scripted_adapter_with(&directory, CodexProvider::OpenAi, "codex-script", &script);
    let mut configuration = adapter.config.clone();
    configuration.local_user = Some(NativeV2CodexUser { home, codex_home });
    let mut adapter = NativeV2CodexAdapter::new_for_test(configuration);
    adapter.test_permission_policy =
        Some(crate::native_v2_capsule::provider_process::PermissionPolicy::Configured);
    let adapter = Arc::new(adapter);
    let mut binding = binding(
        SessionScope::NodeInstance,
        &["CAPTURE_PATH", "CONFIG_CAPTURE", "OPENAI_API_KEY"],
    );
    let NodeRuntimeBinding::Agent { effort, .. } = &mut binding else {
        panic!("expected an agent binding");
    };
    *effort = None;
    let admitted = admitted(binding, CodexProvider::OpenAi).await;
    let runtime = runner(&admitted, adapter);
    let capture_path = directory.child("capture");
    let config_capture = directory.child("config-capture");
    let values = [
        ("CAPTURE_PATH", capture_path.display().to_string()),
        ("CONFIG_CAPTURE", config_capture.display().to_string()),
        ("OPENAI_API_KEY", "fake-litellm-key".to_owned()),
    ];
    for execution in [1, 2] {
        let handle = start(&runtime, &admitted, execution, &values).await;
        let (_, outcome) = complete_with_logs(handle).await;
        assert!(matches!(
            outcome.assert_value(),
            WorkerOutcome::Verified { .. }
        ));
    }
    runtime
        .close_run(&openengine_cluster_protocol::RunId::new("run-codex"))
        .await;

    assert_eq!(fs::read_to_string(config_path).assert_value(), config);
    assert_eq!(
        fs::read_to_string(config_capture).assert_value(),
        config.repeat(2)
    );
    let capture = fs::read_to_string(capture_path).assert_value();
    assert_eq!(capture.matches("arg=resume\n").count(), 1);
    assert_eq!(capture.matches("openai_key=fake-litellm-key\n").count(), 2);
    assert_eq!(capture.matches("codex_key=fake-litellm-key\n").count(), 2);
    for forbidden in [
        "arg=model_provider=",
        "arg=model_providers.",
        "arg=web_search=",
        "arg=sandbox_workspace_write.network_access=",
        "arg=model_reasoning_effort=",
        "arg=approval_policy=",
        "arg=--sandbox",
        "arg=--dangerously-bypass",
    ] {
        assert!(
            !capture.contains(forbidden),
            "unexpected override: {forbidden}"
        );
    }
}
