use super::*;
use crate::native_v2_cli::{BuiltinGraphTemplate, TemplateDelivery};
use openengine_cluster_testkit::assertions::AssertValue;

#[test]
fn serve_api_retains_the_local_one_argument_entry_point() {
    let future = serve("127.0.0.1:0".parse().assert_value());
    drop(future);
}

struct Server {
    url: String,
    task: tokio::task::JoinHandle<()>,
    root: std::path::PathBuf,
    workspace: String,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn state(root: std::path::PathBuf, origin: &str) -> UiState {
    UiState::new(
        LocalRunProfileStore::new(root.clone()),
        runs::NativeRunHistory::new(root.join("state")),
        origin,
        "local",
    )
    .assert_value()
}
async fn server() -> Server {
    let root = std::env::temp_dir().join(format!("zeroshot-ui-test-{}", uuid::Uuid::now_v7()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .assert_value();
    let authority = listener.local_addr().assert_value().to_string();
    let state = state(root.clone(), &format!("http://{authority}"));
    let workspace = state.workspace.id.clone();
    let app = router(state, false);
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.assert_value();
    });
    Server {
        url: format!("http://{authority}"),
        task,
        root,
        workspace,
    }
}
fn profile_request() -> Value {
    json!({
        "name":"browser-test",
        "graph":BuiltinGraphTemplate::SingleWorker
            .materialize(TemplateDelivery::None)
            .assert_value(),
        "runtime":{
            "harness":"codex", "provider":"openai", "size":"small",
            "nodes":{"worker":{"kind":"agent","model":"opaque-model"}}
        },
        "expectedRevision":null
    })
}
#[tokio::test]
async fn direct_profile_handlers_round_trip_without_a_transport() {
    let root = openengine_cluster_testkit::TemporaryDirectory::for_test("profile-ui-handlers");
    let state = state(root.as_path().to_owned(), "https://target.example");
    let workspace = state.workspace.id.clone();

    let Json(bootstrap) = bootstrap(State(state.clone())).await.assert_value();
    assert_eq!(bootstrap["workspace"]["kind"], "local");
    assert_eq!(bootstrap["workspace"]["id"], workspace);
    let Json(empty) = list(State(state.clone())).await.assert_value();
    assert_eq!(empty["profiles"], json!([]));

    let mut headers = HeaderMap::new();
    headers.insert("x-zeroshot-workspace", workspace.parse().assert_value());
    let request = profile_request();
    let Json(saved) = save(
        State(state.clone()),
        headers,
        Ok(Bytes::from(request.to_string())),
    )
    .await
    .assert_value();
    assert_eq!(saved["profile"]["name"], "browser-test");

    let Json(profiles) = list(State(state.clone())).await.assert_value();
    assert_eq!(profiles["profiles"].as_array().assert_value().len(), 1);
    let Json(shown) = show(State(state.clone()), Path("browser-test".to_owned()))
        .await
        .assert_value();
    assert_eq!(shown, saved);
    let error = show(State(state), Path("bad/name".to_owned()))
        .await
        .err()
        .assert_value();
    assert_eq!(error.code, "invalid_profile");
}

#[tokio::test]
async fn direct_authoring_and_validation_handlers_preserve_drafts_and_reject_bad_json() {
    let request = profile_request();
    let document = json!({"graph":request["graph"],"runtime":request["runtime"]});
    let Json(valid) = validate(Ok(Bytes::from(document.to_string())))
        .await
        .assert_value();
    assert_eq!(valid, json!({"valid":true}));

    let data_request = json!({
        "graph": request["graph"],
        "runtime": request["runtime"],
        "action": {
            "kind":"run_input_field", "name":"request",
            "type":{"kind":"string"}, "required":true
        }
    });
    let Json(authored) = data_author(Ok(Bytes::from(data_request.to_string())))
        .await
        .assert_value();
    assert_eq!(
        authored["graph"]["initialInput"]["fields"]["request"]["type"],
        json!({"kind":"string"})
    );

    for malformed in ["{", r#"{"graph":null,"runtime":null,"extra":true}"#] {
        let error = validate(Ok(Bytes::from(malformed.to_owned())))
            .await
            .err()
            .assert_value();
        assert_eq!(error.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error.code, "invalid_profile");
        assert!(error.message.contains("Invalid profile JSON"));
    }
}

#[tokio::test]
async fn helper_failures_keep_stable_problem_categories() {
    struct RefusesSerialization;
    impl serde::Serialize for RefusesSerialization {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            use serde::ser::Error as _;
            Err(S::Error::custom("fixture refusal"))
        }
    }

    let serialization = encoded(RefusesSerialization).err().assert_value();
    assert_eq!(serialization.code, "profile_store_error");
    assert!(serialization.message.contains("fixture refusal"));
    let operation = blocking::<()>(|| Err(NativeV2CliError::Local("store offline".into())))
        .await
        .err()
        .assert_value();
    assert_eq!(operation.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(operation.message.contains("store offline"));
    assert!(
        local_error(std::io::Error::other("disk offline"))
            .to_string()
            .contains("disk offline")
    );

    let mapped: ApiError = workspace::WorkspaceError {
        status: 99,
        code: "fixture",
        message: "invalid status".into(),
    }
    .into();
    assert_eq!(mapped.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(mapped.code, "fixture");
}
#[tokio::test]
async fn browser_save_is_cli_visible_and_conflicts_are_atomic() {
    let server = server().await;
    let client = reqwest::Client::new();
    let url = format!("{}/ui/api/profiles", server.url);
    let request = profile_request();
    let first = client
        .post(&url)
        .header("x-zeroshot-workspace", &server.workspace)
        .json(&request)
        .send()
        .await
        .assert_value();
    assert_eq!(first.status(), StatusCode::OK);
    let saved: Value = first.json().await.assert_value();
    let store = LocalRunProfileStore::new(server.root.clone());
    let shown = store
        .show(RunProfileSelector {
            name: RunProfileName::new("browser-test").assert_value(),
            scope: RunProfileScope::User,
        })
        .assert_value();
    assert_eq!(
        serde_json::to_value(&shown).assert_value(),
        saved["profile"]
    );
    assert_eq!(
        client
            .post(&url)
            .header("x-zeroshot-workspace", &server.workspace)
            .json(&request)
            .send()
            .await
            .assert_value()
            .status(),
        StatusCode::CONFLICT
    );
    let mut update = request;
    update["expectedRevision"] = saved["revision"].clone();
    update["runtime"]["nodes"]["worker"]["model"] = json!("updated-model");
    let (a, b) = tokio::join!(
        client
            .post(&url)
            .header("x-zeroshot-workspace", &server.workspace)
            .json(&update)
            .send(),
        client
            .post(&url)
            .header("x-zeroshot-workspace", &server.workspace)
            .json(&update)
            .send()
    );
    let statuses = [a.assert_value().status(), b.assert_value().status()];
    assert!(statuses.contains(&StatusCode::OK));
    assert!(statuses.contains(&StatusCode::CONFLICT));
}
#[tokio::test]
async fn invalid_profile_never_enters_store() {
    let server = server().await;
    let client = reqwest::Client::new();
    let mut request = profile_request();
    request["runtime"]["nodes"] = json!({});
    let response = client
        .post(format!("{}/ui/api/profiles", server.url))
        .header("x-zeroshot-workspace", &server.workspace)
        .json(&request)
        .send()
        .await
        .assert_value();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let problem: Value = response.json().await.assert_value();
    assert_eq!(problem["code"], "invalid_profile");
    assert!(!server.root.join("profiles.json").exists());
}
#[tokio::test]
async fn profile_writes_require_one_matching_workspace_identity() {
    let server = server().await;
    let client = reqwest::Client::new();
    let url = format!("{}/ui/api/profiles", server.url);
    for identities in [
        vec![],
        vec!["another-workspace"],
        vec![server.workspace.as_str(), server.workspace.as_str()],
    ] {
        let mut request = client.post(&url).json(&profile_request());
        for identity in identities {
            request = request.header("x-zeroshot-workspace", identity);
        }
        let response = request.send().await.assert_value();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let problem: Value = response.json().await.assert_value();
        assert_eq!(problem["code"], "workspace_changed");
        assert!(
            problem["message"]
                .as_str()
                .assert_value()
                .contains("Reload")
        );
        assert!(!server.root.join("profiles.json").exists());
    }
}
#[tokio::test]
async fn a_replaced_store_rejects_an_old_tabs_new_profile_without_a_revision() {
    let server = server().await;
    std::fs::remove_dir_all(&server.root).assert_value();
    let replacement = LocalRunProfileStore::new(server.root.clone());
    let new_identity = replacement.workspace_id().assert_value();
    assert_ne!(new_identity, server.workspace);
    let response = reqwest::Client::new()
        .post(format!("{}/ui/api/profiles", server.url))
        .header("x-zeroshot-workspace", &server.workspace)
        .json(&profile_request())
        .send()
        .await
        .assert_value();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let problem: Value = response.json().await.assert_value();
    assert_eq!(problem["code"], "workspace_changed");
    assert!(!server.root.join("profiles.json").exists());
}
#[tokio::test]
async fn authoring_returns_only_a_draft_and_uses_the_browser_boundary() {
    let server = server().await;
    let client = reqwest::Client::new();
    let url = format!("{}/ui/api/authoring", server.url);
    let mut request = profile_request();
    request.as_object_mut().assert_value().remove("name");
    request
        .as_object_mut()
        .assert_value()
        .remove("expectedRevision");
    request["runtime"] =
        json!({"harness":"","provider":"","nodes":{"worker":{"kind":"agent","model":""}}});
    request["graph"]["root"] = json!({
        "kind":"seq", "name":"run", "state":{"kind":"record","fields":{}},
        "children":[{"kind":"fail","name":"failed","reason":"old_reason"}],
        "promotedStatePaths":[]
    });
    request["action"] =
        json!({"kind":"failure_reason","terminal":"failed","reason":"budget_exhausted"});
    let response = client.post(&url).json(&request).send().await.assert_value();
    assert_eq!(response.status(), StatusCode::OK);
    let result: Value = response.json().await.assert_value();
    assert_eq!(
        result["graph"]["root"]["children"][0]["reason"],
        "budget_exhausted"
    );
    assert_eq!(result["runtime"], request["runtime"]);
    assert!(result.get("profile").is_none());
    assert!(!server.root.join("profiles.json").exists());
    for reason in ["unhandled", "runtime_failed", "runtime_lost"] {
        request["action"]["reason"] = json!(reason);
        assert_eq!(
            client
                .post(&url)
                .json(&request)
                .send()
                .await
                .assert_value()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    assert_eq!(
        client
            .post(&url)
            .header("origin", "https://untrusted.example")
            .json(&request)
            .send()
            .await
            .assert_value()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(!server.root.join("profiles.json").exists());
}
#[tokio::test]
async fn local_browser_boundary_rejects_foreign_origins_and_dns_rebinding() {
    let server = server().await;
    let client = reqwest::Client::new();
    let url = format!("{}/ui/api/profiles", server.url);
    for header in [
        ("origin", "https://untrusted.example"),
        ("host", "untrusted.example"),
        ("sec-fetch-site", "cross-site"),
    ] {
        let response = client
            .get(&url)
            .header(header.0, header.1)
            .send()
            .await
            .assert_value();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let response = client.post(&url).body("{}").send().await.assert_value();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}
#[tokio::test]
async fn static_assets_and_bootstrap_are_native_and_self_contained() {
    let server = server().await;
    let client = reqwest::Client::new();
    let page = client
        .get(format!("{}/ui/", server.url))
        .send()
        .await
        .assert_value();
    assert_eq!(page.status(), StatusCode::OK);
    assert!(
        page.headers()["content-security-policy"]
            .to_str()
            .assert_value()
            .contains("worker-src 'self'")
    );
    assert!(
        page.text()
            .await
            .assert_value()
            .contains("Profiles · Zeroshot")
    );
    let bootstrap: Value = client
        .get(format!("{}/ui/api/bootstrap", server.url))
        .send()
        .await
        .assert_value()
        .json()
        .await
        .assert_value();
    assert_eq!(bootstrap["templates"].as_array().assert_value().len(), 7);
    assert_eq!(bootstrap["workers"].as_array().assert_value().len(), 4);
    assert!(bootstrap["runtimeSchema"]["$defs"].is_object());
}
