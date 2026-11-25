use super::*;

async fn response(
    schema: (&str, bool),
    initial: &str,
    corrected: &str,
    malformed: bool,
) -> (WorkerOutcome, String) {
    let (kind, contained) = schema;
    let directory = TestDirectory::new("codex-filepath");
    let escaped = |path: &str| {
        serde_json::to_string(&serde_json::to_string(path).assert_value()).assert_value()
    };
    let replace_answer = |value: &str| {
        format!(
            r#"\"answer\":{}"#,
            &escaped(value)[1..escaped(value).len() - 1]
        )
    };
    let source = SCRIPT
        .replace(
            "  /usr/bin/printf '%s\\n' '---'\n",
            "  /usr/bin/printf '%s\\n' '---'\n  /usr/bin/printf 'uid=%s\\n' \"$(/usr/bin/id -u)\"\n",
        )
        .replace(r#"\"answer\":42"#, &replace_answer(initial))
        .replace(r#"\"answer\":43"#, &replace_answer(corrected))
        .replace(r#"\"answer\":\"wrong\""#, &replace_answer("missing"));
    let mut adapter = scripted_adapter_with(
        &directory,
        CodexProvider::OpenAi,
        "codex-filepaths",
        &source,
    );
    if contained {
        use std::os::unix::fs::PermissionsExt;
        fs::write(directory.child("capture"), "").expect("create contained fixture capture");
        fs::set_permissions(
            directory.child("capture"),
            fs::Permissions::from_mode(0o666),
        )
        .expect("permit contained fixture capture writes");
        adapter = Arc::new(NativeV2CodexAdapter::new(adapter.config.clone()));
    }
    fs::create_dir_all(directory.child("workspace/report/nested"))
        .expect("create filepath fixture tree");
    fs::write(directory.child("workspace/report/nested/file"), "current")
        .expect("write filepath fixture member");
    let admitted = admitted_with_output(
        binding(
            SessionScope::Execution,
            &["CAPTURE_PATH", "OPENAI_API_KEY", "ALWAYS_MALFORMED"],
        ),
        CodexProvider::OpenAi,
        json!({"kind":kind}),
    )
    .await;
    let runtime = runner(&admitted, adapter);
    let handle = start(
        &runtime,
        &admitted,
        1,
        &[
            (
                "CAPTURE_PATH",
                directory.child("capture").display().to_string(),
            ),
            ("OPENAI_API_KEY", "fake-key".to_owned()),
            ("ALWAYS_MALFORMED", malformed.to_string()),
        ],
    )
    .await;
    let (logs, outcome) = complete_with_logs(handle).await;
    // The harness retains these already-redacted diagnostics for any later fixture assertion.
    eprintln!(
        "filepath fixture: kind={kind}, contained={contained}, initial={initial:?}, \
         corrected={corrected:?}, malformed={malformed}\nprovider outcome: {outcome:?}\n\
         provider logs:\n{logs}"
    );
    let capture = fs::read_to_string(directory.child("capture"))
        .unwrap_or_else(|error| panic!("read filepath fixture capture: {error:?}"));
    let expected_uid = format!(
        "uid={}",
        if contained {
            10_002
        } else {
            unsafe { libc::geteuid() }
        }
    );
    let identities = capture
        .lines()
        .filter(|line| line.starts_with("uid="))
        .collect::<Vec<_>>();
    // Check every actual invocation, including any provider startup retries. Correction counts
    // are checked separately below; they do not determine how many provider processes started.
    assert!(
        !identities.is_empty(),
        "fixture capture:\n{capture}\nprovider logs:\n{logs}"
    );
    assert_eq!(
        identities.len(),
        capture.lines().filter(|line| *line == "arg=exec").count(),
        "fixture capture:\n{capture}\nprovider logs:\n{logs}"
    );
    assert!(
        identities.iter().all(|identity| *identity == expected_uid),
        "provider identities: {identities:?}"
    );
    // A response validates only. The fixture never has a file-store host or publication.
    assert!(!directory.child("store").exists());
    (outcome.expect("filepath fixture provider outcome"), capture)
}

#[tokio::test]
async fn filepath_response_checks_files_folders_confinement_and_correction_limit() {
    check_responses(false).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn root_filepath_response_checks_contained_files_folders_confinement_and_correction_limit() {
    // The native and coverage CI jobs execute root_ selectors with their privileged runner.
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("contained filepath verification requires the root_ test runner");
        return;
    }
    check_responses(true).await;
}

async fn check_responses(contained: bool) {
    for path in ["report", "report/nested/file"] {
        let (outcome, capture) = response(("filepath", contained), "missing", path, false).await;
        assert!(
            matches!(outcome, WorkerOutcome::Verified { output, .. } if output == json!({"answer":path}))
        );
        assert_eq!(capture.matches("arg=resume").count(), 1);
    }
    let (outcome, capture) = response(
        ("filepath", contained),
        "../codex-filepaths",
        "report",
        false,
    )
    .await;
    assert!(matches!(outcome, WorkerOutcome::Verified { .. }));
    assert_eq!(capture.matches("arg=resume").count(), 1);
    let (outcome, capture) = response(("filepath", contained), "missing", "report", true).await;
    assert_eq!(outcome, WorkerOutcome::malformed());
    assert_eq!(capture.matches("arg=resume").count(), 2);
    let (outcome, capture) = response(("string", contained), "missing", "unused", false).await;
    assert!(
        matches!(outcome, WorkerOutcome::Verified { output, .. } if output == json!({"answer":"missing"}))
    );
    assert!(!capture.contains("arg=resume"));
}
