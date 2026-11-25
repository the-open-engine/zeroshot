//! Selected by the portable observation suite's existing wire_contract filter.
mod wire_contract {
    use super::super::*;
    use super::super::runtime_lanes::RecordingTarget;
    use super::super::super::fixtures::{FakeDialer, MemoryRegistry, hosted_target, temp_root};
    use openengine_cluster_protocol::{
        GraphSpec, RuntimePlan, RunProfileName, SAVED_PROFILE_IMPORTS_KIND,
    };
    use zeroshot_engine::native_v2_cli::{
        execute_native_v2_cli, parse_native_v2_args, NeverDetach, oecp::NamedTargetCliBackend,
    };
    use zeroshot_engine::native_v2_admission::profile_expansion::expand_profile_imports;

    fn graph(imports: bool) -> GraphSpec {
        serde_json::from_value(json!({
            "profile":"openengine.graph.full/v1", "initialInput":{"kind":"null"},
            "policy":{"policy":"policy.native-v2@1","default":"deny"},
            "root": if imports { json!({"kind":"seq","name":"root","state":{"kind":"null"},"promotedStatePaths":[],"children":[
                {"kind":"profile","name":"call","profile":"child"},
                {"kind":"succeed","name":"complete","output":{"kind":"null"},"bindings":[]}
            ]}) }
                else { json!({"kind":"succeed","name":"host_done","output":{"kind":"null"},"bindings":[]}) }
        })).assert_value()
    }

    fn runtime() -> RuntimePlan {
        serde_json::from_value(
            json!({"harness":"codex","provider":"openai","size":"host","nodes":{}}),
        )
        .assert_value()
    }

    #[tokio::test]
    async fn saved_profile_import_capability_gates_selector_submission_and_keeps_selection_on_server()
     {
        for workspace_mode in [false, true] {
            for imports in [false, true] {
                for marker in [
                    None,
                    Some("openengine.saved-profile-imports/v2"),
                    Some(SAVED_PROFILE_IMPORTS_KIND),
                ] {
                    let root = temp_root();
                    let target = RecordingTarget::spawn(move |request, origin, tokens| {
                    let response = match (request.method.as_str(), request.path.as_str()) {
                        ("GET", "/.well-known/zeroshot-native-v2") => {
                            let mut doc: serde_json::Value = serde_json::from_str(&hosted_discovery(origin)).assert_value();
                            if let Some(kind) = marker { doc["extensions"]["saved_profile_imports"] = json!({"kind":kind}); }
                            doc["extensions"]["run_workspaces"] = json!({"kind":openengine_cluster_protocol::RUN_WORKSPACES_KIND});
                            doc.to_string()
                        }
                        ("GET", "/oauth/metadata") => oauth_metadata(origin),
                        ("POST", "/oauth/token") => token_response(tokens),
                        ("GET", "/session") => json!({"kind":"openengine.target-session/v1","organization_id":"organization-1"}).to_string(),
                        ("POST", "/native-v2/profiles/list") => json!({"profiles":[{"id":"parent-id","name":"parent","scope":"org","isDefault":false}]}).to_string(),
                        ("POST", "/native-v2/profiles/show") => {
                            let selector: serde_json::Value = serde_json::from_str(&request.body).assert_value();
                            assert_eq!(selector, json!({"scope":"org","name":"parent"}));
                            json!({"id":"parent-id","name":"parent","scope":"org","graph":graph(imports),"runtime":runtime(),"isDefault":false}).to_string()
                        }
                        ("POST", "/native-v2/profiles/run") => {
                            let body: serde_json::Value = serde_json::from_str(&request.body).assert_value();
                            assert_eq!(body["profile"], json!({"scope":"org","name":"parent"}));
                            assert!(body.get("graph").is_none() && body.get("runtime").is_none());
                            if workspace_mode {
                                assert!(body.get("source").is_none(), "workspace mode never infers the invoking Git source");
                                assert_eq!(body["workspace"], json!({"placement":"managed"}));
                                assert_eq!(body["environment"], json!({}));
                            } else {
                                assert_eq!(body["source"]["repository"], "acme/project");
                            }
                            assert_eq!(body["submissionKey"], "profile-import-contract");
                            assert_eq!(request.authorization.as_deref(), Some("Bearer access-1"));
                            // The server chooses its canonical parent and children. Nothing from
                            // the client's preview is used to supply an expanded body.
                            let mut selected = graph(imports);
                            if imports {
                                let parent = RunProfileName::new("parent").assert_value();
                                let mut middle = graph(false);
                                middle.root = serde_json::from_value(json!({"kind":"seq","name":"scope","state":{"kind":"null"},"promotedStatePaths":[],"children":[
                                    {"kind":"profile","name":"nested","profile":"leaf"},
                                    {"kind":"succeed","name":"complete","output":{"kind":"null"},"bindings":[]}
                                ]})).assert_value();
                                let catalog = std::collections::BTreeMap::from([
                                    (RunProfileName::new("child").assert_value(), (middle, runtime())),
                                    (RunProfileName::new("leaf").assert_value(), (graph(false), runtime())),
                                ]);
                                (selected, _) = expand_profile_imports(selected, runtime(), Some(&parent), |name| Ok(catalog.get(name).cloned())).assert_value();
                                assert!(serde_json::to_string(&selected).assert_value().contains("call.nested.host_done"));
                            }
                            let request: openengine_cluster_protocol::RunProfileRunRequest = serde_json::from_str(&request.body).assert_value();
                            zeroshot_engine::native_v2_admission::validate_workspace_mode(request.workspace.as_ref(), &selected, request.environment.as_ref()).assert_value();
                            json!({"runId":"018f5e78-7f95-7c22-8d98-3f15af20c991"}).to_string()
                        }
                        _ => panic!("unexpected client request {} {}", request.method, request.path),
                    };
                    ("200 OK", response)
                }).await;
                    let (credentials, authority) = test_authority(&root);
                    let record = hosted_target("prod", target.origin.clone());
                    credentials
                        .set(&record.id, "refresh-0")
                        .await
                        .assert_value();
                    let registry = MemoryRegistry::default();
                    registry.insert(record).assert_value();
                    let dialer = FakeDialer::default();
                    let backend = NamedTargetCliBackend::new(NativeV2TargetConnector::new(
                        registry,
                        authority,
                        dialer.clone(),
                    ));
                    let workspace = root.path("workspace");
                    let status = std::process::Command::new("git")
                        .args(["init", "-b", "main"])
                        .arg(&workspace)
                        .output()
                        .assert_value();
                    assert!(status.status.success());
                    std::fs::write(root.path("input.json"), b"null").assert_value();
                    let args = [
                        "run",
                        "--target",
                        "prod",
                        "--profile",
                        "org:parent",
                        "--title",
                        "Profile import contract",
                        "--input",
                        root.path("input.json").to_str().assert_value(),
                        "--repository",
                        "acme/project",
                        "--branch",
                        "main",
                        "--revision",
                        "0123456789abcdef0123456789abcdef01234567",
                        "--submission-key",
                        "profile-import-contract",
                        "--cwd",
                        workspace.to_str().assert_value(),
                        "--detach",
                    ]
                    .map(std::ffi::OsString::from);
                    let mut args = args.to_vec();
                    if workspace_mode {
                        for flag in ["--repository", "--branch", "--revision", "--cwd"] {
                            let index = args.iter().position(|arg| arg == flag).assert_value();
                            args.drain(index..index + 2);
                        }
                        args.extend(
                            ["--workspace", "--no-environment"].map(std::ffi::OsString::from),
                        );
                    }
                    let mut validation_args = args.to_vec();
                    validation_args.push("--validate-only".into());
                    let validation = execute_native_v2_cli(
                        parse_native_v2_args(validation_args).assert_value(),
                        &backend,
                        &mut NeverDetach,
                        &mut Vec::new(),
                    )
                    .await;
                    if imports {
                        let message = validation.err().assert_value().to_string();
                        assert!(
                            message.contains(if marker == Some(SAVED_PROFILE_IMPORTS_KIND) {
                                "require host admission"
                            } else {
                                SAVED_PROFILE_IMPORTS_KIND
                            })
                        );
                    } else {
                        validation.assert_value();
                    }
                    let command = parse_native_v2_args(args).assert_value();
                    let result =
                        execute_native_v2_cli(command, &backend, &mut NeverDetach, &mut Vec::new())
                            .await;
                    let requests = target.stop();
                    let supported = !imports || marker == Some(SAVED_PROFILE_IMPORTS_KIND);
                    if supported {
                        result.assert_value();
                    } else {
                        assert!(
                            result
                                .err()
                                .assert_value()
                                .to_string()
                                .contains(SAVED_PROFILE_IMPORTS_KIND)
                        );
                    }
                    assert_eq!(
                        requests
                            .iter()
                            .filter(|r| r.path == "/native-v2/profiles/run")
                            .count(),
                        usize::from(supported)
                    );
                    assert_eq!(
                        requests
                            .iter()
                            .filter(|r| r.path == "/native-v2/profiles/show")
                            .count(),
                        2,
                        "each preview reads only the selected parent, never child profiles"
                    );
                    assert!(
                        !requests
                            .iter()
                            .any(|r| r.path == "/native-v2/run"
                                || r.path == "/native-v2/oecp-session")
                    );
                    assert!(dialer.sessions.lock().assert_value().is_empty());
                }
            }
        }
    }
}
