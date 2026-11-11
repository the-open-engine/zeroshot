//! Portable subprocess fixtures exercise the adapter's real process supervisor without a shell.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;

use fs2::FileExt;

use super::*;
use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::HostedProcessScope;
use crate::execution::WorkspaceAccessMode;
use crate::native_v2_candidate::test_support::TestDirectory;
use crate::native_v2_runner::{DurableNodeEvent, test_support};

const FIXTURE_TEST: &str = "native_v2_codex::process::tests::settlement::codex_process_fixture";
const MODE: &str = "ZEROSHOT_CODEX_SETTLEMENT_FIXTURE";
const ROOT: &str = "ZEROSHOT_CODEX_SETTLEMENT_ROOT";
const SECRET: &str = "private-teardown-token";

#[test]
fn codex_process_fixture() {
    let Ok(mode) = std::env::var(MODE) else {
        return;
    };
    let root = std::env::var_os(ROOT).assert_value();
    let root = Path::new(&root);
    if mode == "descendant" {
        let lock = File::create(root.join("descendant.lock")).assert_value();
        lock.lock_exclusive().assert_value();
        fs::write(root.join("descendant.ready"), b"ready").assert_value();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    if mode != "blocked-input" {
        let mut input = Vec::new();
        std::io::stdin().read_to_end(&mut input).assert_value();
    }
    let mut descendant = None;
    if mode == "hang" {
        descendant = Some(
            Command::new(std::env::current_exe().assert_value())
                .args(["--exact", FIXTURE_TEST, "--nocapture"])
                .env(MODE, "descendant")
                .spawn()
                .assert_value(),
        );
        let end = std::time::Instant::now() + Duration::from_secs(5);
        while !root.join("descendant.ready").exists() {
            assert!(std::time::Instant::now() < end, "descendant did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    println!("{{\"type\":\"thread.started\",\"thread_id\":\"settled-thread\"}}");
    println!(
        "{{\"type\":\"item.completed\",\"item\":{{\"type\":\"agent_message\",\"text\":\"done\"}}}}"
    );
    println!(
        "{{\"type\":\"turn.completed\",\"usage\":{{\"input_tokens\":17,\"output_tokens\":9}}}}"
    );
    std::io::stdout().flush().assert_value();
    if mode != "clean" {
        eprintln!("teardown detail {SECRET}");
    }
    match mode.as_str() {
        "clean" => std::process::exit(0),
        "exit" => std::process::exit(1),
        "signal" => {
            #[cfg(unix)]
            // SAFETY: this dedicated fixture process terminates only itself.
            unsafe {
                libc::raise(libc::SIGKILL);
            }
            #[cfg(windows)]
            // SAFETY: the pseudo-handle denotes only this dedicated fixture process.
            unsafe {
                use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
                TerminateProcess(GetCurrentProcess(), 1);
            }
            panic!("fixture termination returned");
        }
        "closed-stdout" => {
            #[cfg(unix)]
            // SAFETY: only the fixture's stdout is closed; its process deliberately stays alive.
            unsafe {
                libc::close(libc::STDOUT_FILENO);
            }
            #[cfg(windows)]
            // SAFETY: only the fixture's standard output handle is closed.
            unsafe {
                use windows_sys::Win32::{
                    Foundation::CloseHandle,
                    System::Console::{GetStdHandle, STD_OUTPUT_HANDLE},
                };
                CloseHandle(GetStdHandle(STD_OUTPUT_HANDLE));
            }
        }
        "hang" | "blocked-input" => {}
        _ => panic!("unknown fixture mode"),
    }
    if let Some(mut child) = descendant {
        let _ = child.wait();
    }
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

async fn open_fixture(
    directory: &TestDirectory,
    control: &DriverControl,
    mode: &str,
) -> ProviderProcess {
    let files = Arc::new(
        ProviderExecutionFiles::for_test(directory.path(), HostedProcessScope::WriterExecution(1))
            .assert_value(),
    );
    let command = ProcessSessionCommand {
        program: std::env::current_exe()
            .assert_value()
            .to_str()
            .assert_value()
            .to_owned(),
        argv: vec![
            "--exact".to_owned(),
            FIXTURE_TEST.to_owned(),
            "--nocapture".to_owned(),
        ],
        environment: BTreeMap::from([
            (MODE.to_owned(), mode.to_owned()),
            (
                ROOT.to_owned(),
                directory.path().to_str().assert_value().to_owned(),
            ),
        ]),
        workspace: WorkspaceCapability {
            current_dir: directory.path().to_owned(),
            mode: WorkspaceAccessMode::ReadWrite,
        },
        deadline: None,
    };
    match open_process(files, command, control).await.assert_value() {
        ProcessOpen::Ready(process) => process,
        ProcessOpen::ProviderFailure(detail) => panic!("fixture launch failed: {detail}"),
    }
}

#[tokio::test]
async fn completed_turn_settles_with_usage_and_one_redacted_teardown_log() {
    for mode in ["clean", "exit", "signal", "hang", "closed-stdout"] {
        let directory = TestDirectory::new("codex-completed-settlement");
        let (_cancel, control, mut durable) = test_support::control_with_capacity(16);
        let mut process = open_fixture(&directory, &control, mode).await;
        let session = CodexSession::new();
        let output = tokio::time::timeout(
            Duration::from_secs(15),
            exchange_turn(
                &mut process,
                "prompt",
                &ProcessTurnContext {
                    control: &control,
                    session: &session,
                    resumed: false,
                },
                &[SECRET.to_owned()],
            ),
        )
        .await
        .assert_value_with("completed turn must settle without process exit")
        .assert_value();
        assert_eq!(output.final_message().assert_value(), "done");
        assert!(output.failure_message().is_none());
        let completion = process.wait().await.assert_value();
        assert!(completion.cleanup.proves_tree_empty());
        if mode == "hang" {
            let lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(directory.child("descendant.lock"))
                .assert_value();
            lock.try_lock_exclusive()
                .assert_value_with("descendant must release its lock before completion");
        }
        drop(control);
        let mut diagnostics = Vec::new();
        let mut usages = Vec::new();
        while let Ok(event) = durable.recv().await {
            match event {
                DurableNodeEvent::Output { output, .. }
                    if output.stream == LiveOutputStream::Error =>
                {
                    diagnostics.push(output.text)
                }
                DurableNodeEvent::TokenUsage(usage) => usages.push(usage),
                _ => {}
            }
        }
        assert_eq!(usages.len(), 1);
        let usage = usages[0].assert_value();
        assert_eq!(usage.input_tokens.get(), 17);
        assert_eq!(usage.output_tokens.get(), 9);
        if mode == "clean" {
            assert!(diagnostics.is_empty());
        } else {
            assert_eq!(diagnostics.len(), 1, "{mode}: {diagnostics:?}");
            assert!(diagnostics[0].contains("teardown failed after the completed turn"));
            assert!(diagnostics[0].contains("teardown detail [REDACTED]"));
            assert!(!diagnostics[0].contains(SECRET));
        }
    }
}

#[tokio::test]
async fn completed_turn_bounds_stalled_input_and_keeps_input_failure_fatal() {
    let directory = TestDirectory::new("codex-stalled-completed-input");
    let (_cancel, control, _durable) = test_support::control_with_capacity(16);
    let mut process = open_fixture(&directory, &control, "blocked-input").await;
    let session = CodexSession::new();
    let prompt = "x".repeat(9 * 1024 * 1024);
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        exchange_turn(
            &mut process,
            &prompt,
            &ProcessTurnContext {
                control: &control,
                session: &session,
                resumed: false,
            },
            &[],
        ),
    )
    .await
    .assert_value_with("stalled stdin must remain bounded")
    .assert_value();
    assert!(
        output
            .failure_message()
            .assert_value()
            .contains("provider process input")
    );
    assert!(
        process
            .wait()
            .await
            .assert_value()
            .cleanup
            .proves_tree_empty()
    );
}

#[tokio::test]
async fn cancellation_after_completed_turn_still_confirms_cleanup() {
    let directory = TestDirectory::new("codex-cancel-completed-settlement");
    let (cancel, control, mut durable) = test_support::control_with_capacity(16);
    let mut process = open_fixture(&directory, &control, "hang").await;
    let session = CodexSession::new();
    let context = ProcessTurnContext {
        control: &control,
        session: &session,
        resumed: false,
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(
            exchange_turn(&mut process, "prompt", &context, &[]),
            async {
                loop {
                    let output = durable.recv_output().await.assert_value();
                    if output.text == "Codex turn completed" {
                        cancel.send_replace(true);
                        break;
                    }
                }
            },
        )
    })
    .await
    .assert_value_with("cancellation must settle after confirmed cleanup");
    assert!(matches!(result, Err(NodeRunnerError::Cancelled)));
    assert!(
        process
            .wait()
            .await
            .assert_value()
            .cleanup
            .proves_tree_empty()
    );
    drop(control);
    assert_eq!(
        durable
            .recv_usage()
            .await
            .assert_value()
            .assert_value()
            .input_tokens
            .get(),
        17
    );
    assert!(durable.recv().await.is_err());
}
