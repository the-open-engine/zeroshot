use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{DuplexStream, ReadHalf, WriteHalf};
use tokio::time::Instant;

use super::*;
use crate::native_v2_cli::{
    CliOutcome, DetachSignal, NeverDetach, execute_native_v2_cli, parse_native_v2_args,
};

type TestTransport = NdjsonTransport<ReadHalf<DuplexStream>, WriteHalf<DuplexStream>>;

/// Connect always succeeds; only the actual OECP subscription acknowledgement fails or stalls.
struct OutageTransport {
    first: TestTransport,
    pending_ack: bool,
    requests: Mutex<Vec<(Instant, Value)>>,
    active_setups: AtomicUsize,
    unary_requests: AtomicUsize,
}

struct SetupGuard<'a>(&'a AtomicUsize);

impl Drop for SetupGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl JsonRpcTransport for OutageTransport {
    async fn request(&self, _: String) -> Result<String, TransportError> {
        self.unary_requests.fetch_add(1, Ordering::SeqCst);
        Err(TransportError::Protocol(
            "unexpected unary request".to_owned(),
        ))
    }
}

#[async_trait]
impl SubscriptionTransport for OutageTransport {
    async fn open_subscription(
        &self,
        request: String,
        id: RequestId,
    ) -> Result<(String, Option<PumpedSubscription>), TransportError> {
        self.active_setups.fetch_add(1, Ordering::SeqCst);
        let _guard = SetupGuard(&self.active_setups);
        let first = {
            let mut requests = self.requests.lock().assert_value();
            requests.push((
                Instant::now(),
                serde_json::from_str(&request).assert_value(),
            ));
            requests.len() == 1
        };
        if first {
            return self.first.open_subscription(request, id).await;
        }
        if self.pending_ack {
            std::future::pending().await
        } else {
            Err(TransportError::Protocol(
                "subscription acknowledgement lost".to_owned(),
            ))
        }
    }

    async fn cancel_subscription(&self, _: SubscriptionId) -> Result<(), TransportError> {
        Ok(())
    }

    async fn cancel_request(&self, _: RequestId) -> Result<(), TransportError> {
        Ok(())
    }

    fn next_watch_request_id(&self) -> RequestId {
        self.first.next_watch_request_id()
    }
}

fn outage_backend(
    command: &str,
    pending_ack: bool,
) -> (
    NamedTargetCliBackend<StubConnector<OutageTransport>>,
    Arc<OutageTransport>,
) {
    let (client, server) = tokio::io::duplex(16 * 1024);
    let (client_read, client_write) = tokio::io::split(client);
    let command = command.to_owned();
    tokio::spawn(async move {
        let (read, mut write) = tokio::io::split(server);
        let mut request = String::new();
        BufReader::new(read)
            .read_line(&mut request)
            .await
            .assert_value();
        let request: Value = serde_json::from_str(&request).assert_value();
        let mut result = json!({"subscriptionId":"first","runId":"run-contract","atCursor":"v2:0"});
        if command == "attach" {
            result =
                json!({"subscriptionId":"first","runId":"run-contract","execution":"worker-1"});
        }
        let response = json!({"jsonrpc":"2.0","id":request["id"],"result":result});
        write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .assert_value();
        let mut event = match command.as_str() {
            "watch" => serde_json::to_value(status("admitted")).assert_value(),
            "logs" => json!({
                "runId":"run-contract", "timestamp":1725000000123_u64,
                "record":{"level":"info","target":"agent","message":"before disconnect"}
            }),
            "attach" => json!({
                "runId":"run-contract","execution":"worker-1",
                "event":{"type":"output","text":"before disconnect"}
            }),
            _ => unreachable!(),
        };
        event.as_object_mut().assert_value().remove("atCursor");
        event["subscriptionId"] = json!("first");
        if command != "attach" {
            event["cursor"] = json!("v2:1");
        }
        let notification = json!({"jsonrpc":"2.0","method":"event","params":event});
        write
            .write_all(format!("{notification}\n").as_bytes())
            .await
            .assert_value();
        write.shutdown().await.assert_value();
    });
    let transport = Arc::new(OutageTransport {
        first: NdjsonTransport::new(client_read, client_write),
        pending_ack,
        requests: Mutex::new(Vec::new()),
        active_setups: AtomicUsize::new(0),
        unary_requests: AtomicUsize::new(0),
    });
    (
        NamedTargetCliBackend::new(StubConnector {
            transport: transport.clone(),
            hosted_status: None,
        }),
        transport,
    )
}

struct DetachAfter(Duration);

#[async_trait]
impl DetachSignal for DetachAfter {
    async fn wait(&mut self) {
        tokio::time::sleep(self.0).await;
    }
}

#[tokio::test(start_paused = true)]
async fn named_target_recovery_waits_for_acknowledgement_and_aborts_pending_setup() {
    for command in ["watch", "logs", "attach"] {
        for pending_ack in [false, true] {
            for detach in [false, true] {
                let (backend, transport) = outage_backend(command, pending_ack);
                let mut args = vec![command, "run-contract", "--target", "prod"];
                if command == "attach" {
                    args.push("worker-1");
                }
                let parsed = parse_native_v2_args(args.into_iter().map(std::ffi::OsString::from))
                    .assert_value();
                let mut output = Vec::new();
                let start = Instant::now();
                let result = if detach {
                    execute_native_v2_cli(
                        parsed,
                        &backend,
                        &mut DetachAfter(Duration::from_secs(8)),
                        &mut output,
                    )
                    .await
                } else {
                    execute_native_v2_cli(parsed, &backend, &mut NeverDetach, &mut output).await
                };
                if detach {
                    assert_eq!(result.assert_value(), CliOutcome::Detached);
                    assert_eq!(start.elapsed(), Duration::from_secs(8));
                } else {
                    let error = result.assert_error();
                    assert!(
                        matches!(error, NativeV2CliError::FollowConnectionLost { .. }),
                        "{error}"
                    );
                    assert!(
                        error
                            .to_string()
                            .contains("zeroshot watch run-contract --target prod")
                    );
                    assert_eq!(start.elapsed(), Duration::from_secs(600));
                }
                tokio::task::yield_now().await;
                assert_eq!(transport.active_setups.load(Ordering::SeqCst), 0);
                assert_eq!(transport.unary_requests.load(Ordering::SeqCst), 0);
                let requests = transport.requests.lock().assert_value();
                assert!(requests.len() >= 2);
                for (_, request) in requests.iter().skip(1) {
                    assert_eq!(request["params"]["runId"], "run-contract");
                    if command == "attach" {
                        assert_eq!(request["params"]["execution"], "worker-1");
                    } else {
                        assert_eq!(request["params"]["fromCursor"], "v2:1");
                    }
                }
                if !pending_ack {
                    let delays = requests
                        .windows(2)
                        .map(|pair| pair[1].0 - pair[0].0)
                        .collect::<Vec<_>>();
                    assert_eq!(
                        &delays[..5],
                        &[
                            Duration::from_millis(100),
                            Duration::from_millis(200),
                            Duration::from_millis(400),
                            Duration::from_millis(800),
                            Duration::from_millis(1600),
                        ]
                    );
                    assert!(delays.iter().all(|delay| *delay <= Duration::from_secs(5)));
                }
                assert_eq!(String::from_utf8(output).assert_value().lines().count(), 1);
            }
        }
    }
}
