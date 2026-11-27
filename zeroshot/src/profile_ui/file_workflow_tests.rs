use super::*;

#[tokio::test]
async fn filepath_input_edits_preserve_request_and_final_publications_through_save() {
    let server = server().await;
    let client = reqwest::Client::new();
    let catalog = crate::workspace::catalog().assert_value();
    let mut ask = catalog["workers"]
        .as_array()
        .assert_value()
        .iter()
        .find(|worker| worker["id"] == "request_ask")
        .assert_value()["node"]
        .clone();
    ask["name"] = json!("approval");
    ask["inputBindings"] = json!([{"target":["text"],"value":{"source":"state","path":["text"]}}]);
    let publications = json!({
        "selected":{"kind":"binding","value":{"source":"state","path":["source"]}},
        "other":{"kind":"binding","value":{"source":"state","path":["other"]}},
        "literal":{"kind":"literal","path":"source"}
    });
    ask["artifacts"] = publications.clone();
    let input = json!({"kind":"record","fields":{
        "source":{"type":{"kind":"filepath"},"required":true},
        "other":{"type":{"kind":"filepath"},"required":true},
        "text":{"type":{"kind":"string"},"required":true}
    }});
    let mut request = json!({
        "name":"file-publications", "expectedRevision":null,
        "graph":{
            "profile":"openengine.graph.full/v1", "initialInput":input,
            "policy":{"policy":"policy.native-v2@1","default":"deny"},
            "root":{"kind":"seq","name":"run","state":input,
                "promotedStatePaths":[],"children":[ask,{
                    "kind":"succeed","name":"done","artifacts":publications,
                    "output":{"kind":"record","fields":{"file":{"type":{"kind":"filepath"},"required":true}}},
                    "bindings":[{"target":["file"],"value":{"source":"state","path":["source"]}}]
                }]}
        },
        "runtime":{"harness":"codex","provider":"openai","nodes":{"approval":{"kind":"request"}}}
    });
    // Normalize wire defaults before comparing saved and authored graphs.
    request["graph"] = serde_json::to_value(
        serde_json::from_value::<GraphSpec>(request["graph"].clone()).assert_value(),
    )
    .assert_value();
    let initial = client
        .post(format!("{}/ui/api/validate", server.url))
        .json(&json!({"graph":request["graph"],"runtime":request["runtime"]}))
        .send()
        .await
        .assert_value();
    assert_eq!(
        initial.status(),
        StatusCode::OK,
        "{}",
        initial.text().await.assert_value()
    );
    for (renaming, action) in [
        (
            true,
            json!({"kind":"run_input_field","before":"source","name":"renamed","type":{"kind":"filepath"},"required":true}),
        ),
        (false, json!({"kind":"remove_run_input","name":"renamed"})),
    ] {
        let response = client
            .post(format!("{}/ui/api/data", server.url))
            .json(&json!({"graph":request["graph"],"runtime":request["runtime"],"action":action}))
            .send()
            .await
            .assert_value();
        assert_eq!(response.status(), StatusCode::OK);
        let document: Value = response.json().await.assert_value();
        for node in document["graph"]["root"]["children"]
            .as_array()
            .assert_value()
        {
            if renaming {
                assert_eq!(
                    node["artifacts"]["selected"]["value"]["path"],
                    json!(["renamed"])
                );
            } else {
                assert!(node["artifacts"].get("selected").is_none());
            }
            assert_eq!(node["artifacts"]["other"], publications["other"]);
            assert_eq!(node["artifacts"]["literal"], publications["literal"]);
        }
        let validation = client
            .post(format!("{}/ui/api/validate", server.url))
            .json(&document)
            .send()
            .await
            .assert_value();
        assert_eq!(
            validation.status(),
            StatusCode::OK,
            "{}",
            validation.text().await.assert_value()
        );
        request["graph"] = document["graph"].clone();
        request["runtime"] = document["runtime"].clone();
        let response = client
            .post(format!("{}/ui/api/profiles", server.url))
            .header("x-zeroshot-workspace", &server.workspace)
            .json(&request)
            .send()
            .await
            .assert_value();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{}",
            response.text().await.assert_value()
        );
        let shown: Value = client
            .get(format!("{}/ui/api/profiles/file-publications", server.url))
            .send()
            .await
            .assert_value()
            .json()
            .await
            .assert_value();
        assert_eq!(shown["profile"]["graph"], request["graph"]);
        request["graph"] = shown["profile"]["graph"].clone();
        request["expectedRevision"] = shown["revision"].clone();
    }
}
