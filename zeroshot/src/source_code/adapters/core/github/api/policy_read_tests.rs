use super::*;
use super::super::policy_read::transient;
use serde_json::json;

const MESSAGE: &str = "Something went wrong while executing your query on 2026-11-21T18:29:14Z. Please include `2790:35FB30:99F13F9:E90DBDA:6B01E2F9` when reporting this issue.";

fn native_error(message: &str) -> ApiOutput {
    ApiOutput {
        stdout: json!([{"errors":[{"message":message}]}])
            .to_string()
            .into_bytes(),
        stderr: format!("gh: {message}\n").into_bytes(),
        ..Default::default()
    }
}

#[test]
fn policy_read_exhaustion_is_typed_and_survives_error_context_and_retry_hints() {
    for status in [None, Some(503)] {
        let error = SourceDeliveryError::api(status, "read failed")
            .exhausted()
            .with_context("mutation failed")
            .temporary()
            .temporary_review_sync();
        assert!(error.retries_exhausted());
        assert!(!error.retryable_operation());
        assert!(!error.retryable_review_sync());
        assert_eq!(error.api_status(), status);
    }
    let error = SourceDeliveryError::api(Some(503), "retry allowance exhausted");
    assert!(!error.retries_exhausted());
    assert!(error.retryable_operation());
}

#[test]
fn policy_read_classification_requires_complete_unmixed_native_evidence() {
    assert!(transient(&native_error(MESSAGE)));
    assert!(transient(&native_error(
        &MESSAGE
            .replace("2026-11-21T18:29:14Z", "2027-01-02T03:04:05Z")
            .replace(
                "2790:35FB30:99F13F9:E90DBDA:6B01E2F9",
                "different-request-id"
            )
    )));
    let error = json!({"errors":[{"message":MESSAGE}]});
    let success = json!({"data":{"repository":{"nameWithOwner":"acme/project"}}});
    assert!(transient(&ApiOutput {
        stdout: json!([success, error]).to_string().into_bytes(),
        ..native_error(MESSAGE)
    }));
    for value in [
        Value::Null,
        json!([]),
        error.clone(),
        json!([{"errors":[]}]),
        json!([{"errors":[{"message":"unknown"}]}]),
        json!([{"errors":[{"message":MESSAGE},{"message":"permission denied"}]}]),
        json!([{"data":null,"errors":[{"message":MESSAGE}]}]),
        json!([{"data":{"repository":{}},"errors":[{"message":MESSAGE}]}]),
        json!([{"errors":[{"message":MESSAGE,"type":"UNPROCESSABLE"}]}]),
        json!([{"errors":[{"message":MESSAGE}],"status":503}]),
        json!([error, success]),
        json!([error, error]),
        json!([{}, error]),
    ] {
        assert!(!transient(&ApiOutput {
            stdout: value.to_string().into_bytes(),
            ..native_error(MESSAGE)
        }));
    }
    let output = native_error(MESSAGE);
    for stdout in [
        output.stdout[..output.stdout.len() - 1].to_vec(),
        [output.stdout.as_slice(), b"trailing"].concat(),
        [output.stdout.as_slice(), &[0xff]].concat(),
        [output.stdout.as_slice(), &vec![b' '; MAX_API_OUTPUT_BYTES]].concat(),
    ] {
        assert!(!transient(&ApiOutput {
            stdout,
            ..native_error(MESSAGE)
        }));
    }
    for stderr in [
        Vec::new(),
        b"gh: unknown error\n".to_vec(),
        [
            output.stderr.as_slice(),
            b"gh: permission denied (HTTP 403)\n",
        ]
        .concat(),
        [output.stderr.as_slice(), &[0xff]].concat(),
        [output.stderr.as_slice(), &vec![b' '; MAX_API_ERROR_BYTES]].concat(),
    ] {
        assert!(!transient(&ApiOutput {
            stderr,
            ..native_error(MESSAGE)
        }));
    }
    for message in [
        "Something went wrong while executing your query",
        "Something went wrong while executing your query on . Please include `id` when reporting this issue.",
        "Something went wrong while executing your query on date. Please include `` when reporting this issue.",
        "Something went wrong while executing your query on date. Please include `id` when reporting this issue. extra",
        "Something went wrong while executing your query on date. Please include `id\nextra` when reporting this issue.",
    ] {
        assert!(!transient(&native_error(message)));
    }
}

#[test]
fn policy_read_classification_precedes_credential_redaction_and_diagnostic_bounds() {
    for token in [
        "errors",
        "message",
        "Something went wrong",
        "2790:35FB30",
        "2026-11-21",
    ] {
        let mut output = native_error(MESSAGE);
        let error: Value = serde_json::from_slice(&output.stdout).unwrap();
        output.stdout =
            json!([{"data":{"padding":"x".repeat(MAX_API_DIAGNOSTIC_STREAM_BYTES)}},error[0]])
                .to_string()
                .into_bytes();
        assert!(transient(&output));
        let failure = output.failure(
            Some(std::process::ExitStatus::default()),
            "read failed",
            SourceDeliveryCredential::token(token),
        );
        assert!(!failure.to_string().contains(token));
        assert_eq!(failure.api_status(), None);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn policy_read_transient_signals_do_not_opt_other_api_callers_into_retries() {
    let root = TemporaryDirectory::for_test("github-policy-scope");
    let output = native_error(MESSAGE);
    std::fs::write(root.as_path().join("stdout"), output.stdout).unwrap();
    std::fs::write(root.as_path().join("stderr"), output.stderr).unwrap();
    let program = script(
        root.as_path(),
        "printf 'call\\n' >> \"$HOME/calls\"\ncat \"$HOME/stdout\"\ncat \"$HOME/stderr\" >&2\nexit 1\n",
    );
    let error = authority(program, Duration::from_secs(10))
        .api(
            &["graphql".to_owned()],
            SourceDeliveryCredential::token("test-token"),
        )
        .await
        .unwrap_err();
    assert_eq!(error.api_status(), None);
    assert!(!error.retryable_operation());
    assert!(!error.to_string().contains("retry allowance"));
    assert_eq!(
        std::fs::read_to_string(root.as_path().join("calls")).unwrap(),
        "call\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn incomplete_native_policy_error_never_acquires_retry_authority() {
    for ending in [
        "exec sleep 30",
        "printf '\\377'",
        "head -c 67108865 /dev/zero",
        "head -c 65537 /dev/zero >&2",
    ] {
        let root = TemporaryDirectory::for_test("github-policy-incomplete");
        let output = native_error(MESSAGE);
        std::fs::write(root.as_path().join("stdout"), output.stdout).unwrap();
        std::fs::write(root.as_path().join("stderr"), output.stderr).unwrap();
        let program = script(
            root.as_path(),
            &format!("cat \"$HOME/stdout\"\ncat \"$HOME/stderr\" >&2\n{ending}\nexit 1\n"),
        );
        let provider = authority(program, Duration::from_millis(200));
        let failure = provider
            .request_api_output(
                &["graphql".to_owned()],
                SourceDeliveryCredential::token("test-token"),
                ApiCapture::Standard,
            )
            .await
            .err()
            .unwrap();
        assert!(!failure.policy_read_transient);
        assert!(!failure.error.to_string().contains("retry allowance"));
        assert_eq!(failure.error.api_status(), None);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn policy_read_timeout_retains_bounded_redacted_streams_and_command_context() {
    for attempt in [1, 2] {
        let root = TemporaryDirectory::for_test("github-policy-timeout");
        let output = native_error(MESSAGE);
        std::fs::write(root.as_path().join("stdout"), output.stdout).unwrap();
        std::fs::write(root.as_path().join("stderr"), output.stderr).unwrap();
        for (stream, prefix) in [
            ("partial-stdout", "partial stdout"),
            ("partial-stderr", "gh: partial response (HTTP 401)"),
        ] {
            std::fs::write(
                root.as_path().join(stream),
                format!(
                    "{prefix} test-token private-proxy-password\n{}",
                    "padding\n".repeat(MAX_API_DIAGNOSTIC_STREAM_BYTES)
                ),
            )
            .unwrap();
        }
        let program = script(
            root.as_path(),
            &format!(
                r#"
printf 'call\n' >> "$HOME/calls"
count=$(/usr/bin/wc -l < "$HOME/calls")
if [ "$count" -lt {attempt} ]; then
    cat "$HOME/stdout"
    cat "$HOME/stderr" >&2
    exit 1
fi
cat "$HOME/partial-stdout"
cat "$HOME/partial-stderr" >&2
exec /bin/sleep 30
"#
            ),
        );
        let network =
            crate::source_code_provider::adapters::core::network::ForgeNetwork::from_snapshot(
                &BTreeMap::from([(
                    "HTTPS_PROXY".to_owned(),
                    "http://private-proxy-user:private-proxy-password@proxy.invalid:8080"
                        .to_owned(),
                )]),
            );
        let started = std::time::Instant::now();
        let error = authority(program, Duration::from_millis(400 + (attempt - 1) * 1000))
            .with_network(network)
            .policy_api(
                &["graphql".to_owned(), "query=diagnostic-query".to_owned()],
                SourceDeliveryCredential::token("test-token"),
            )
            .await
            .unwrap_err();
        let diagnostic = error.to_string();
        assert!(diagnostic.contains("command:"));
        assert!(diagnostic.contains("query=diagnostic-query"));
        assert!(diagnostic.contains("command timed out"));
        assert!(diagnostic.contains("stdout (truncated=true):\npartial stdout [REDACTED]"));
        assert!(diagnostic.contains("stderr (truncated=true):\ngh: partial response (HTTP 401)"));
        assert!(!diagnostic.contains("test-token"));
        assert!(!diagnostic.contains("private-proxy-password"));
        assert!(
            diagnostic.len() <= crate::source_code_provider::MAX_SOURCE_API_DIAGNOSTIC_BYTES + 32
        );
        assert_eq!(error.api_status(), None);
        assert!(!error.authentication_failed());
        assert_eq!(error.retries_exhausted(), attempt > 1);
        assert_eq!(error.retryable_operation(), attempt == 1);
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(
            std::fs::read_to_string(root.as_path().join("calls"))
                .unwrap()
                .lines()
                .count(),
            attempt as usize
        );
    }
}

// The existing macOS forge lane selects api::tests, including this native retry regression.
#[cfg(unix)]
#[tokio::test]
async fn policy_read_native_recovery_and_exhaustion_are_operation_scoped() {
    for failures in [2, 99] {
        let root = TemporaryDirectory::for_test("github-policy-retry");
        let output = native_error(MESSAGE);
        std::fs::write(root.as_path().join("stdout"), output.stdout).unwrap();
        std::fs::write(root.as_path().join("stderr"), output.stderr).unwrap();
        let program = script(
            root.as_path(),
            &format!(
                r#"
printf 'call\n' >> "$HOME/calls"
count=$(/usr/bin/wc -l < "$HOME/calls")
if [ "$count" -le {failures} ]; then
    cat "$HOME/stdout"
    cat "$HOME/stderr" >&2
    exit 1
fi
printf '[{{"data":{{"complete":true}}}}]'
"#
            ),
        );
        let network =
            crate::source_code_provider::adapters::core::network::ForgeNetwork::from_snapshot(
                &BTreeMap::from([(
                    "HTTPS_PROXY".to_owned(),
                    "http://private-proxy-user:allowance@proxy.invalid:8080".to_owned(),
                )]),
            );
        let result = authority(program, Duration::from_secs(30))
            .with_network(network)
            .policy_api(
                &["graphql".to_owned()],
                SourceDeliveryCredential::token("test-token"),
            )
            .await;
        if failures == 2 {
            assert_eq!(result.unwrap(), json!([{"data":{"complete":true}}]));
        } else {
            let error = result.unwrap_err();
            assert!(error.to_string().contains("exhausted after 3 attempts"));
            assert!(!error.to_string().contains("allowance"));
            assert!(!error.retryable_operation());
        }
        assert_eq!(
            std::fs::read_to_string(root.as_path().join("calls"))
                .unwrap()
                .lines()
                .count(),
            3
        );
    }
}
