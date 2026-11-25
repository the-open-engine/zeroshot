use std::ffi::OsString;

use super::*;
use crate::native_v2_cli::{CliOutcome, NeverDetach, execute_native_v2_cli, parse_native_v2_args};

async fn command<C: TargetConnector>(
    backend: &NamedTargetCliBackend<C>,
    args: &[&str],
) -> (Result<CliOutcome, NativeV2CliError>, Vec<u8>) {
    let command = parse_native_v2_args(args.iter().map(OsString::from)).assert_value();
    let mut output = Vec::new();
    let result = execute_native_v2_cli(command, backend, &mut NeverDetach, &mut output).await;
    (result, output)
}

fn assert_selected_once(reads: &Mutex<Vec<(String, RunStatusParams)>>, part: &str) {
    assert_eq!(
        reads.lock().assert_value().as_slice(),
        &[(
            "prod".to_owned(),
            RunStatusParams {
                run_id: RunId::new("run-contract"),
                part_id: Some(RunId::new(part)),
            }
        )]
    );
}

#[tokio::test]
async fn named_steer_routes_to_observed_part_and_checks_the_receipt_before_printing() {
    for reply_part in ["second-part", "wrong-part"] {
        let (client_write, server_read) = tokio::io::duplex(8192);
        let (mut server_write, client_read) = tokio::io::duplex(8192);
        let mut active = status("admitted");
        active.part_id = RunId::new("second-part");
        active.current_part_id = active.part_id.clone();
        let backend = NamedTargetCliBackend::new(StubConnector {
            transport: Arc::new(NdjsonTransport::new(client_read, client_write)),
            hosted_status: Some(active),
            status_reads: Mutex::default(),
        });
        let peer = tokio::spawn(async move {
            let mut read = BufReader::new(server_read);
            let mut line = String::new();
            read.read_line(&mut line).await.assert_value();
            let request: Value = serde_json::from_str(&line).assert_value();
            let receipt = json!({"runId":reply_part,"partId":reply_part,
                "steer":{"id":"direction-1","mode":"once","node":"reviewer","createdAt":1,
                "textBytes":5,"excerpt":"pause","receivedExecutions":0,"executions":[]}});
            let response = json!({"jsonrpc":"2.0","id":request["id"],"result":receipt});
            server_write
                .write_all(format!("{response}\n").as_bytes())
                .await
                .assert_value();
            request
        });
        let (result, output) = command(
            &backend,
            &[
                "steer",
                "run-contract",
                "pause",
                "--target",
                "prod",
                "--part",
                "second-part",
                "--node",
                "reviewer",
                "--mode",
                "once",
            ],
        )
        .await;
        let rpc = peer.await.assert_value();
        assert_eq!(rpc["method"], "run/steer");
        assert_eq!(
            rpc["params"],
            json!({"runId":"second-part","partId":"second-part",
            "node":"reviewer","mode":"once","text":"pause"})
        );
        assert_selected_once(&backend.connector.status_reads, "second-part");
        if reply_part == "second-part" {
            assert_eq!(result.assert_value(), CliOutcome::Completed);
            let receipt: Value = serde_json::from_slice(&output).assert_value();
            assert_eq!(receipt["runId"], "run-contract");
            assert_eq!(receipt["partId"], "second-part");
            assert_eq!(receipt["steer"]["id"], "direction-1");
        } else {
            assert!(matches!(
                result.assert_error(),
                NativeV2CliError::Protocol(_)
            ));
            assert!(output.is_empty());
        }
    }
}

#[tokio::test]
async fn stale_steer_and_queued_suspend_send_no_capsule_action() {
    for (args, phase, part, code) in [
        (
            vec![
                "steer",
                "run-contract",
                "pause",
                "--target",
                "prod",
                "--part",
                "old-part",
            ],
            "admitted",
            "old-part",
            openengine_cluster_protocol::RUN_CONFLICT,
        ),
        (
            vec![
                "suspend",
                "run-contract",
                "--target",
                "prod",
                "--part",
                "head-part",
            ],
            "queued",
            "head-part",
            "invalid_request",
        ),
    ] {
        let mut observed = status(phase);
        observed.part_id = RunId::new(part);
        observed.current_part_id = RunId::new("head-part");
        let backend = NamedTargetCliBackend::new(StubConnector::new(Some(observed)));
        let (result, output) = command(&backend, &args).await;
        let NativeV2CliError::Remote {
            code: actual,
            details,
            ..
        } = result.assert_error()
        else {
            panic!("expected controller refusal");
        };
        assert_eq!(actual, code);
        if code == openengine_cluster_protocol::RUN_CONFLICT {
            assert_eq!(details, Some(json!({"currentPartId":"head-part"})));
        }
        assert!(output.is_empty());
        assert!(
            backend
                .connector
                .transport
                .requests
                .lock()
                .assert_value()
                .is_empty()
        );
        assert_selected_once(&backend.connector.status_reads, part);
    }
}
