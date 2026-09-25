use super::*;

async fn save_resource(server: &Server, collection: &str, body: &Value) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/ui/api/{collection}", server.url))
        .header("x-zeroshot-workspace", &server.workspace)
        .json(body)
        .send()
        .await
        .assert_value()
}

async fn delete_environment(server: &Server, resource: &Value) -> reqwest::Response {
    reqwest::Client::new()
        .delete(format!(
            "{}/ui/api/environments/{}",
            server.url,
            resource["id"].as_str().unwrap()
        ))
        .header("x-zeroshot-workspace", &server.workspace)
        .json(&json!({"expectedRevision":resource["revision"]}))
        .send()
        .await
        .assert_value()
}

#[tokio::test]
async fn environment_http_resources_share_profile_references_and_protect_updates() {
    let server = server().await;
    let first = save_resource(
        &server,
        "environments",
        &json!({"name":"shared", "definition":{"startup":"echo first"}, "expectedRevision":null}),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let resource: Value = first.json().await.assert_value();
    let client = reqwest::Client::new();
    let listed: Value = client
        .get(format!("{}/ui/api/environments", server.url))
        .send()
        .await
        .assert_value()
        .json()
        .await
        .assert_value();
    assert_eq!(listed["environments"][0]["id"], resource["id"]);
    for name in ["one", "two"] {
        let mut profile = profile_request();
        profile["name"] = json!(name);
        profile["runtime"]["environment"] = json!({"id":resource["id"]});
        let response = save_resource(&server, "profiles", &profile).await;
        assert_eq!(response.status(), StatusCode::OK);
        let saved: Value = response.json().await.assert_value();
        assert_eq!(
            saved["profile"]["runtime"]["environment"],
            json!({"id":resource["id"]})
        );
    }
    let edited = json!({
        "id":resource["id"], "name":"renamed", "definition":{"startup":"echo next"},
        "expectedRevision":resource["revision"]
    });
    let response = save_resource(&server, "environments", &edited).await;
    assert_eq!(response.status(), StatusCode::OK);
    let current: Value = response.json().await.assert_value();
    assert_ne!(current["revision"], resource["revision"]);
    assert_eq!(
        save_resource(&server, "environments", &edited)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let conflict = delete_environment(&server, &current).await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let problem: Value = conflict.json().await.assert_value();
    assert_eq!(problem["code"], "environment_in_use");
    let store = LocalRunProfileStore::new(server.root.clone());
    for name in ["one", "two"] {
        let profile = store
            .resolve_profile(RunProfileSelector {
                scope: RunProfileScope::User,
                name: RunProfileName::new(name).assert_value(),
            })
            .assert_value();
        assert_eq!(
            profile.runtime.environment().unwrap().startup.as_deref(),
            Some("echo next")
        );
    }
}

#[tokio::test]
async fn environment_http_missing_resources_and_replaced_workspaces_are_explicit() {
    let server = server().await;
    let client = reqwest::Client::new();
    let url = format!("{}/ui/api/environments", server.url);
    let missing = client
        .get(format!("{url}/missing"))
        .send()
        .await
        .assert_value();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let request = json!({"name":"shared", "definition":{}, "expectedRevision":null});
    assert_eq!(
        client
            .post(&url)
            .json(&request)
            .send()
            .await
            .assert_value()
            .status(),
        StatusCode::CONFLICT
    );
    let malformed = json!({"name":"shared", "definition":{}});
    assert_eq!(
        save_resource(&server, "environments", &malformed)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let reserved = json!({"name":"shared", "definition":{"variables":{"HOME":"override"}}, "expectedRevision":null});
    assert_eq!(
        save_resource(&server, "environments", &reserved)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let mut profile = profile_request();
    profile["runtime"]["environment"] = json!({"id":"missing"});
    let response = save_resource(&server, "profiles", &profile).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn graph_authoring_preserves_environment_reference() {
    let request = profile_request();
    let reference = json!({"id":"shared-reference"});
    let mut runtime = request["runtime"].clone();
    runtime["environment"] = reference.clone();
    let Json(authored) = data_author(Ok(Bytes::from(json!({
        "graph":request["graph"], "runtime":runtime,
        "action":{"kind":"run_input_field", "name":"request", "type":{"kind":"string"}, "required":true}
    }).to_string()))).await.assert_value();
    assert_eq!(authored["runtime"]["environment"], reference);
}

#[tokio::test]
async fn environment_http_accepts_escaped_values_within_decoded_budgets() {
    let server = server().await;
    let script = "\u{1}".repeat(64 * 1024);
    let variables = "\u{1}".repeat(256 * 1024 - "PUBLIC".len());
    let request = json!({
        "name":"escaped", "expectedRevision":null,
        "definition":{"setup":script, "startup":script, "variables":{"PUBLIC":variables}}
    });
    let encoded = serde_json::to_vec(&request).assert_value();
    assert!(encoded.len() > MAX_BODY && encoded.len() < MAX_ENVIRONMENT_BODY);
    let response = save_resource(&server, "environments", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.bytes().await.assert_value();
    assert!(bytes.len() < MAX_ENVIRONMENT_BODY);
    let resource: Value = serde_json::from_slice(&bytes).assert_value();
    assert_eq!(resource["definition"], request["definition"]);
    let shown: Value = reqwest::Client::new()
        .get(format!(
            "{}/ui/api/environments/{}",
            server.url,
            resource["id"].as_str().unwrap()
        ))
        .send()
        .await
        .assert_value()
        .json()
        .await
        .assert_value();
    assert_eq!(shown, resource);
    let profile = save_resource(&server, "profiles", &request).await;
    assert_eq!(profile.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let problem: Value = profile.json().await.assert_value();
    assert!(problem["message"].as_str().unwrap().contains("body limit"));
    let oversized = json!({"definition":{"startup":"x".repeat(MAX_ENVIRONMENT_BODY)}});
    let rejected = save_resource(&server, "environments", &oversized).await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let problem: Value = rejected.json().await.assert_value();
    assert!(problem["message"].as_str().unwrap().contains("body limit"));
}
