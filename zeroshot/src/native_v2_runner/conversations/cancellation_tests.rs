use super::*;
use openengine_cluster_testkit::assertions::AssertValue;
use std::time::{Duration, Instant};

pub(crate) fn large_source(root: &Path) -> ConversationSource {
    fs::create_dir_all(root).assert_value();
    fs::write(root.join("first-thread.jsonl"), b"first turn").assert_value();
    fs::File::create(root.join("large-thread.jsonl"))
        .assert_value()
        .set_len(128 * 1024 * 1024)
        .assert_value();
    crate::native_v2_codex::conversation::layout(root.parent().assert_value().to_owned())
        .capture(ConversationMetadata {
            harness: "codex".to_owned(),
            id: "thread".to_owned(),
            usage: layout_tests::usage(),
        })
        .assert_value()
}

// Observe real writes, then interrupt between bounded reads. No clock-dependent sleep or root
// privilege is needed. The generous source size keeps the copy active until observation.
pub(crate) fn interrupt_copy(
    directory: PathBuf,
    interrupt: impl FnOnce() + Send + 'static,
) -> std::thread::JoinHandle<u64> {
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(entries) = fs::read_dir(&directory) {
                for entry in entries.flatten() {
                    let file = entry.path().join("large-thread.jsonl");
                    if let Ok(metadata) = fs::metadata(file) {
                        if metadata.len() > 0 {
                            interrupt();
                            assert!(
                                metadata.len() < 128 * 1024 * 1024,
                                "copy completed before interruption"
                            );
                            return metadata.len();
                        }
                    }
                }
            }
            assert!(Instant::now() < deadline, "conversation copy never started");
            std::thread::yield_now();
        }
    })
}

pub(crate) fn cancelled_capture_is_omitted() {
    let root = tempfile::tempdir().assert_value();
    let source = large_source(&root.path().join("provider/sessions"));
    let stage = tempfile::tempdir().assert_value();
    let operation = ConversationIo::default();
    let stop = operation.clone();
    let watcher = interrupt_copy(stage.path().join("1"), move || stop.cancel());
    let captured = capture_in(
        stage,
        ConversationSources::from([(
            NodeInstanceId::new(1).assert_value(),
            (NodeName::new("worker").assert_value(), Ok(source)),
        )]),
        &operation,
    );
    assert!(watcher.join().assert_value() > 0);
    assert_eq!(captured.notices[0].1, "conversation copying was cancelled");
    assert_eq!(
        fs::read_dir(captured.files.assert_value().directory())
            .assert_value()
            .count(),
        0
    );
}

#[test]
fn cancellation_during_capture_removes_every_partial_file() {
    cancelled_capture_is_omitted();
}

pub(crate) fn cancelled_restore_is_not_installed() {
    let root = tempfile::tempdir().assert_value();
    let source = large_source(&root.path().join("old/sessions"));
    let layout = crate::native_v2_codex::conversation::layout(root.path().join("new"));
    fs::create_dir_all(&layout.storage).assert_value();
    fs::write(
        layout.storage.join("first-thread.jsonl"),
        b"existing conversation",
    )
    .assert_value();
    let operation = ConversationIo::default();
    let stop = operation.clone();
    let watcher = interrupt_copy(layout.storage.clone(), move || stop.cancel());
    assert_eq!(
        layout.restore_until_stopped(&source, None, &operation),
        Err("conversation copying was cancelled")
    );
    assert!(watcher.join().assert_value() > 0);
    assert_eq!(
        fs::read(layout.storage.join("first-thread.jsonl")).assert_value(),
        b"existing conversation"
    );
    assert_eq!(fs::read_dir(&layout.storage).assert_value().count(), 1);
}

#[test]
fn cancellation_during_restore_keeps_the_existing_conversation() {
    cancelled_restore_is_not_installed();
}

#[derive(Clone)]
struct CapturedSession {
    source: ConversationSource,
    closed: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl crate::native_v2_runner::NodeSession for CapturedSession {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn is_live(&self) -> bool {
        !self.closed.load(std::sync::atomic::Ordering::Acquire)
    }
    async fn close(&self) {
        fs::remove_dir_all(&self.source.storage).assert_value();
        self.closed
            .store(true, std::sync::atomic::Ordering::Release);
    }
    async fn conversation(&self) -> Result<ConversationSource, &'static str> {
        Ok(self.source.clone())
    }
}

#[async_trait::async_trait]
impl crate::native_v2_runner::SessionFactory for CapturedSession {
    async fn open(
        &self,
        _invocation: &crate::native_v2_contract::NodeInvocation,
        _environment: &crate::native_v2_runner::ResolvedEnvironment,
    ) -> Result<
        Arc<dyn crate::native_v2_runner::NodeSession>,
        crate::native_v2_runner::NodeRunnerError,
    > {
        Ok(Arc::new(self.clone()))
    }
}

#[async_trait::async_trait]
impl crate::native_v2_runner::NodeDriver for CapturedSession {
    async fn run(
        &self,
        _invocation: crate::native_v2_runner::DriverInvocation,
        _control: crate::native_v2_runner::DriverControl,
    ) -> Result<openengine_cluster_protocol::WorkerOutcome, crate::native_v2_runner::NodeRunnerError>
    {
        Ok(openengine_cluster_protocol::WorkerOutcome::Verified {
            output: serde_json::Value::Null,
            artifacts: Vec::new(),
        })
    }
}

#[tokio::test]
async fn cancelled_or_abandoned_capture_drains_before_closing_provider_homes() {
    use crate::native_v2_runner::{NativeNodeRunner, NodeRunner};
    use crate::native_v2_runner::test_support::{admitted, request};
    use crate::execution::driver::DriverCancellation;
    for trigger in ["controller cancellation", "session close", "dropped future"] {
        let root = tempfile::tempdir().assert_value();
        let mut source = large_source(&root.path().join("provider/sessions"));
        // The random name identifies this copy among parallel tests' private temporary stages.
        let unique = format!("{}-thread.jsonl", uuid::Uuid::now_v7().simple());
        fs::rename(
            source.storage.join("large-thread.jsonl"),
            source.storage.join(&unique),
        )
        .assert_value();
        source.files = vec![PathBuf::from("first-thread.jsonl"), PathBuf::from(&unique)];
        let provider = Arc::new(CapturedSession {
            source,
            closed: Arc::default(),
        });
        let runner = Arc::new(
            NativeNodeRunner::new(&admitted(), provider.clone(), provider.clone()).assert_value(),
        );
        let mut handle = runner
            .start(request("run", "looped", (1, 1)))
            .await
            .assert_value();
        handle.completion().await.assert_value();
        let (stop, stopping) = tokio::sync::watch::channel(false);
        let capture = {
            let runner = runner.clone();
            tokio::spawn(async move {
                runner
                    .capture_conversations_until_stopped(
                        &openengine_cluster_protocol::RunId::new("run"),
                        DriverCancellation::new(stopping),
                    )
                    .await
            })
        };
        let abort = capture.abort_handle();
        let (started, observed) = tokio::sync::oneshot::channel();
        let watcher = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                for entry in fs::read_dir(std::env::temp_dir()).assert_value().flatten() {
                    if !entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("zeroshot-conversations-")
                    {
                        continue;
                    }
                    let conversation = entry.path().join("1");
                    if fs::metadata(conversation.join("files").join(&unique))
                        .is_ok_and(|meta| meta.len() > 0)
                    {
                        if trigger == "dropped future" {
                            abort.abort();
                        } else if trigger == "controller cancellation" {
                            stop.send_replace(true);
                        }
                        let _ = started.send(conversation);
                        return;
                    }
                }
                assert!(Instant::now() < deadline, "capture never started copying");
                std::thread::yield_now();
            }
        });
        let partial = observed.await.assert_value();
        tokio::time::timeout(
            Duration::from_secs(2),
            runner.close_run(&openengine_cluster_protocol::RunId::new("run")),
        )
        .await
        .assert_value();
        assert!(provider.closed.load(std::sync::atomic::Ordering::Acquire));
        assert!(
            !partial.exists(),
            "close returned before the partial checkpoint was removed"
        );
        if trigger != "dropped future" {
            assert_eq!(
                capture.await.assert_value().notices[0].1,
                "conversation copying was cancelled"
            );
        } else {
            assert!(matches!(capture.await, Err(error) if error.is_cancelled()));
        }
        watcher.join().assert_value();
    }
}
