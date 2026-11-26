use super::*;
use crate::native_v2_files::{FileAccess, FilePin, WorkspaceFiles, pack};

fn upload(files: &WorkspaceFiles, source: &std::path::Path, operation: &str) -> Value {
    let pin = FilePin {
        run_id: RunId::new("upload"),
        operation: IdempotencyKey::new(operation).unwrap(),
    };
    let mut transfer = Vec::new();
    pack(source, &mut transfer).unwrap();
    files
        .host
        .stage_upload(
            &FileAccess {
                target: &files.target,
                identity: &files.identity,
            },
            &pin,
            &mut std::io::Cursor::new(transfer),
        )
        .unwrap();
    json!({"kind":"staged", "runId":pin.run_id, "operation":pin.operation})
}

#[tokio::test]
async fn named_answer_preflight_reclaims_only_declared_uploads_and_preserves_accepted_pins() {
    for mode in ["stale-part", "successor", "scalar-only"] {
        let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let state = temp.path().join("state");
        crate::execution::platform::create_private_directory(&state).unwrap();
        let files = crate::native_v2_cli::files::workspace_files(&state).unwrap();
        let source = temp.path().join("data.csv");
        std::fs::write(&source, "accepted bytes").unwrap();
        let accepted = upload(&files, &source, "accepted");
        let schema = serde_json::from_value(json!({"kind":"record", "fields":{
            "reply":{"required":false,"type":{"kind":"filepath"}},
            "kind":{"required":false,"type":{"kind":"string"}},
            "runId":{"required":false,"type":{"kind":"string"}},
            "operation":{"required":false,"type":{"kind":"string"}}
        }}))
        .unwrap();
        let accepted_output = json!({"reply":accepted});
        let owner = RunId::new("winner");
        let frozen = files
            .freeze_answer_inputs(crate::native_v2_files::inputs::InputAdmission {
                run_id: &owner,
                owner: &owner,
                key: &IdempotencyKey::new("accepted").unwrap(),
                schema: &schema,
                value: &accepted_output,
                local_root: None,
            })
            .unwrap();
        files
            .release_staged_inputs(&schema, &accepted_output)
            .unwrap();
        std::fs::write(&source, "losing bytes").unwrap();
        let losing = upload(&files, &source, "losing");
        std::fs::write(&source, "unrelated scalar upload").unwrap();
        let scalar = upload(&files, &source, "scalar");
        let mut output = scalar.clone();
        if mode != "scalar-only" {
            output["reply"] = losing.clone();
        }
        let answer = temp.path().join("answer.json");
        std::fs::write(
            &answer,
            serde_json::to_vec(&json!({"signals":{},"output":output})).unwrap(),
        )
        .unwrap();
        let request = json!({"id":"question", "text":"Select a file", "signals":{}, "output":schema,"expiresAt":4102444800000_u64});
        let status = serde_json::from_value(json!({"partId":"original", "currentPartId":if mode=="stale-part" {"winner"} else {"original"},
            "runId":"original", "title":"Question", "size":"host", "atCursor":"v2:1", "status":{"phase":"suspended","reason":"request", "node":"question", "createdAt":1,"request":request}})).unwrap();
        let mut connector = StubConnector::new(Some(status));
        if mode != "stale-part" {
            connector
                .hosted_status
                .as_mut()
                .unwrap()
                .workspace_recovery
                .successor_run_id = Some(owner.clone());
        }
        connector.answer_files = Some((
            serde_json::from_value(
                json!({"runId":"original", "partId":"original", "node":"question", "createdAt":1,
            "request":request, "state":"answered", "successorPartId":"winner"}),
            )
            .unwrap(),
            files,
        ));
        let backend = NamedTargetCliBackend::new(connector);
        let files = &backend.connector.answer_files.as_ref().unwrap().1;
        let count = || {
            std::fs::read_dir(state.join("files/objects"))
                .unwrap()
                .count()
        };
        assert_eq!(count(), 3);
        let mut receipt = Vec::new();
        let result = super::super::super::execute_native_v2_cli(
            super::super::super::NativeV2CliCommand::Answer {
                run: super::super::super::RunSelector {
                    target: Some("remote".into()),
                    run_id: RunId::new("original"),
                    part_id: (mode == "stale-part").then(|| RunId::new("original")),
                    controller: Default::default(),
                },
                input: answer,
                forward_github_token: false,
            },
            &backend,
            &mut super::super::super::NeverDetach,
            &mut receipt,
        )
        .await;
        let error = result.expect_err("already answered").to_string();
        assert!(
            error.contains(if mode == "stale-part" {
                "RUN_CONFLICT"
            } else {
                "already has successor"
            }),
            "{error}"
        );
        assert!(receipt.is_empty(), "refusal emits no successor");
        assert_eq!(count(), if mode == "scalar-only" { 3 } else { 2 });
        assert!(
            backend
                .connector
                .transport
                .requests
                .lock()
                .unwrap()
                .is_empty(),
            "no successor RPC"
        );
        files.materialize_file_inputs(&owner, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(frozen.value["reply"].as_str().unwrap()).unwrap(),
            "accepted bytes"
        );
        // Only the scalar upload and (for the scalar-only control) the unselected upload remain.
        files
            .release_staged_inputs(&schema, &json!({"reply":scalar}))
            .unwrap();
        files
            .release_staged_inputs(&schema, &json!({"reply":losing}))
            .unwrap();
        assert_eq!(count(), 1);
        files.release_file_inputs(&owner).unwrap();
        assert_eq!(count(), 0);
        assert_eq!(
            std::fs::read_to_string(source).unwrap(),
            "unrelated scalar upload"
        );
    }
}
