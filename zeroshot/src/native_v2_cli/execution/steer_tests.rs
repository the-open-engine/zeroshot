use super::*;
use crate::native_v2_cli::tests::support::FakeBackend;
use crate::native_v2_cli::{NeverDetach, SteerText};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

#[test]
fn steer_text_sources_preserve_utf8_and_reject_invalid_or_unreadable_input() {
    let directory = tempfile::tempdir().assert_value();
    let path = directory.path().join("direction.txt");
    let maximum = "é".repeat(openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES / 2);
    for text in ["  Keep\tthis\r\n方向 exactly.  ", maximum.as_str()] {
        std::fs::write(&path, text).assert_value();
        for source in [SteerText::Text(text.into()), SteerText::File(path.clone())] {
            assert_eq!(read_steer_text(source).assert_value(), text);
        }
    }
    for (bytes, diagnostic) in [
        (
            vec![b'x'; openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES + 1],
            "exceeds",
        ),
        (vec![0xff], "UTF-8"),
        (b" \t\r\n".to_vec(), "non-whitespace"),
        (b"before\0after".to_vec(), "NUL"),
        (b"before\x1bafter".to_vec(), "control"),
    ] {
        std::fs::write(&path, bytes).assert_value();
        let error = read_steer_text(SteerText::File(path.clone())).assert_error();
        assert!(matches!(error, NativeV2CliError::Usage(_)), "{error}");
        assert!(error.to_string().contains(diagnostic), "{error}");
    }
    std::fs::remove_file(&path).assert_value();
    let error = read_steer_text(SteerText::File(path)).assert_error();
    assert!(error.to_string().contains("cannot read steer text file"));
    // Opening a directory succeeds on Unix, but reading it must still report a usage error.
    #[cfg(unix)]
    assert!(
        read_steer_text(SteerText::File(directory.path().into()))
            .assert_error()
            .to_string()
            .contains("cannot read steer text:")
    );
}

#[test]
fn steer_stdin_is_bounded_and_validated_without_changing_text() {
    const CHILD: &str = "ZEROSHOT_STEER_STDIN_TEST";
    if let Ok(expected) = std::env::var(CHILD) {
        let result = read_steer_text(SteerText::Stdin);
        if expected == "valid" {
            assert_eq!(result.assert_value(), "  Follow\t方向\r\nexactly.  ");
        } else {
            assert!(result.assert_error().to_string().contains(&expected));
        }
        return;
    }
    for (bytes, expected) in [
        ("  Follow\t方向\r\nexactly.  ".as_bytes().to_vec(), "valid"),
        (
            vec![b'x'; openengine_cluster_protocol::MAX_NODE_INSTRUCTIONS_BYTES + 1],
            "exceeds",
        ),
        (vec![0xff], "UTF-8"),
    ] {
        use std::process::{Command, Stdio};
        let mut child = Command::new(std::env::current_exe().assert_value())
            .args(["--exact", "native_v2_cli::execution::steer_tests::steer_stdin_is_bounded_and_validated_without_changing_text", "--nocapture"])
            .env(CHILD, expected)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().assert_value();
        child
            .stdin
            .take()
            .assert_value()
            .write_all(&bytes)
            .assert_value();
        let output = child.wait_with_output().assert_value();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[tokio::test]
async fn steer_default_backend_refuses_without_output_and_validation_precedes_dispatch() {
    let backend = FakeBackend::default();
    for (text, diagnostic) in [
        ("direction", "target does not advertise steering"),
        ("\0", "NUL"),
    ] {
        let command = NativeV2CliCommand::Steer {
            run: RunSelector {
                target: Some("older-target".into()),
                run_id: RunId::new("run"),
            },
            text: SteerText::Text(text.into()),
            networks: vec![],
            mode: openengine_cluster_protocol::SteeringMode::Standing,
        };
        let mut output = Vec::new();
        let error = execute_native_v2_cli(command, &backend, &mut NeverDetach, &mut output)
            .await
            .assert_error();
        assert!(error.to_string().contains(diagnostic), "{error}");
        assert!(output.is_empty());
        assert!(backend.calls().is_empty());
    }
}
