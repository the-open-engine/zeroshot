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
        (b" \t\r\n".to_vec(), "steer must contain a non-whitespace"),
        (b"before\0after".to_vec(), "steer must not contain NUL"),
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
async fn steer_node_uses_human_authorization_and_rejects_network_even_without_parser() {
    let mut command = NativeV2CliCommand::Steer {
        run: RunSelector {
            controller: Default::default(),
            target: None,
            part_id: None,
            run_id: RunId::new("run"),
        },
        text: SteerText::Text("direction".into()),
        node: Some(openengine_cluster_protocol::NodeName::new("worker").assert_value()),
        networks: vec![],
        mode: Default::default(),
    };
    for label in ["ZEROSHOT_EXECUTION_REF", "ZEROSHOT_STEERING_CAPABILITY"] {
        let error = steering::try_publish(&command, &|name| (name == label).then(|| "set".into()))
            .await
            .assert_error();
        assert!(error.to_string().contains("authorized --network"));
    }
    if let NativeV2CliCommand::Steer { networks, .. } = &mut command {
        networks.push(openengine_cluster_protocol::NodeName::new("team").assert_value());
    }
    let error = steering::try_publish(&command, &|_| None)
        .await
        .assert_error();
    assert!(
        error
            .to_string()
            .contains("--node cannot be combined with --network")
    );
}

#[tokio::test]
async fn steer_default_backend_refuses_without_output_and_validation_precedes_dispatch() {
    let backend = FakeBackend::default();
    for (text, diagnostic) in [
        ("direction", "target does not advertise steering"),
        ("\0", "steer must not contain NUL"),
    ] {
        let command = NativeV2CliCommand::Steer {
            node: None,
            run: RunSelector {
                controller: Default::default(),
                target: Some("older-target".into()),
                part_id: None,
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

#[tokio::test]
async fn network_publication_never_falls_back_to_a_human_control() {
    use crate::native_v2_cli::parse_native_v2_args;
    use std::ffi::OsString;

    let directory = tempfile::tempdir().assert_value();
    let capability = directory.path().join("expired-capability");
    for (target, has_capability, expected) in [
        (Some("prod"), true, "network messages cannot use --target"),
        (None, false, "no network publishing mandate"),
        (None, true, "network publication failed"),
    ] {
        let mut command = parse_native_v2_args(
            ["steer", "second-part", "hold", "--network", "review"]
                .into_iter()
                .map(OsString::from),
        )
        .assert_value();
        // The parser already refuses a named target. Library callers can supply a command directly.
        if let NativeV2CliCommand::Steer { run, .. } = &mut command {
            run.target = target.map(str::to_owned);
        }
        let backend = FakeBackend::default();
        let env = |name: &str| {
            (has_capability && name == "ZEROSHOT_STEERING_CAPABILITY")
                .then(|| capability.clone().into_os_string())
        };
        let mut output = Vec::new();
        let error = execute_native_v2_cli_with_context(
            command,
            &CliExecutionContext::new(&backend, &env),
            &mut NeverDetach,
            &mut output,
        )
        .await
        .assert_error();
        assert!(error.to_string().contains(expected), "{error}");
        assert!(output.is_empty());
        assert!(backend.calls().is_empty());
    }
}

#[tokio::test]
async fn network_publication_returns_the_originating_parts_receipt_without_human_dispatch() {
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .assert_value();
    let capability = tempfile::NamedTempFile::new().assert_value();
    let address = listener.local_addr().assert_value();
    std::fs::write(
        capability.path(),
        json!({"endpoint":address,"token":"fixture"}).to_string(),
    )
    .assert_value();
    let receipt = json!({"runId":"worker-part","partId":"worker-part","steer":{
        "id":"peer-direction","mode":"once","createdAt":1,"textBytes":5,"excerpt":"pause",
        "receivedExecutions":0,"executions":[]}});
    let response = serde_json::to_vec(&json!({"Ok":receipt})).assert_value();
    let publication = tokio::spawn(async move {
        let (mut connection, _) = listener.accept().await.assert_value();
        let length = connection.read_u32().await.assert_value();
        assert!(length < 4096);
        let mut body = vec![0; length as usize];
        connection.read_exact(&mut body).await.assert_value();
        connection
            .write_u32(u32::try_from(response.len()).assert_value())
            .await
            .assert_value();
        connection.write_all(&response).await.assert_value();
        serde_json::from_slice::<Value>(&body).assert_value()
    });
    let command = |part: &str| {
        crate::native_v2_cli::parse_native_v2_args(
            [
                "steer",
                "worker-part",
                "pause",
                "--network",
                "review",
                "--part",
                part,
            ]
            .into_iter()
            .map(std::ffi::OsString::from),
        )
        .assert_value()
    };
    let backend = FakeBackend::default();
    let env = |name: &str| {
        (name == "ZEROSHOT_STEERING_CAPABILITY").then(|| capability.path().as_os_str().to_owned())
    };
    let context = CliExecutionContext::new(&backend, &env);
    let mut output = Vec::new();
    let error = execute_native_v2_cli_with_context(
        command("prior-part"),
        &context,
        &mut NeverDetach,
        &mut output,
    )
    .await
    .assert_error();
    assert!(
        matches!(error, NativeV2CliError::Usage(message) if message.contains("--part to match"))
    );
    assert!(output.is_empty());
    let result = execute_native_v2_cli_with_context(
        command("worker-part"),
        &context,
        &mut NeverDetach,
        &mut output,
    )
    .await
    .assert_value();
    assert_eq!(result, CliOutcome::Completed);
    assert_eq!(
        serde_json::from_slice::<Value>(&output).assert_value(),
        receipt
    );
    assert_eq!(
        publication.await.assert_value(),
        json!({"token":"fixture","operation":"publish",
        "params":{"runId":"worker-part","text":"pause","mode":"once","networks":["review"]}})
    );
    assert!(backend.calls().is_empty());
}
