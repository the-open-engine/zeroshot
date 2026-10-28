use super::*;
use openengine_cluster_testkit::assertions::AssertValue;
use crate::source_code_provider::SourceGitCredential;

const GIT_FIXTURE_BUDGET: Duration = Duration::from_secs(60);

#[tokio::test]
async fn review_checkout_fetches_pr_head_and_retains_diverged_base() {
    let repository = Repository::new();
    git(&repository.workspace, &["checkout", "-b", "review-head"]);
    std::fs::write(repository.workspace.join("change"), "reviewed change").assert_value();
    git(&repository.workspace, &["add", "--all"]);
    git(&repository.workspace, &["commit", "-m", "review head"]);
    let head = git(&repository.workspace, &["rev-parse", "HEAD"]);
    let remote = repository.remote.to_str().assert_value();
    git(
        &repository.workspace,
        &["push", remote, "HEAD:refs/pull/17/head"],
    );
    git(&repository.workspace, &["checkout", "main"]);
    std::fs::write(repository.workspace.join("upstream"), "new base").assert_value();
    git(&repository.workspace, &["add", "--all"]);
    git(&repository.workspace, &["commit", "-m", "advance base"]);
    let base = git(&repository.workspace, &["rev-parse", "HEAD"]);
    git(&repository.workspace, &["push", remote, "main"]);
    let checkout = repository.root.path().join("review-checkout");
    std::fs::create_dir(&checkout).assert_value();
    let transport = repository
        .transport(Path::new("git"), &checkout, repository.remote.as_os_str())
        .with_checkout_references(vec!["refs/pull/17/head".to_owned(), base.clone()])
        .assert_value();
    transport
        .checkout("main", &head, SourceDeliveryCredential::token(""))
        .await
        .assert_value();
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        git(&checkout, &["rev-parse", "refs/zeroshot/review/base"]),
        base
    );
    assert_eq!(
        git(&checkout, &["merge-base", "HEAD", &base]),
        repository.base
    );
    assert!(
        transport
            .with_checkout_references(vec!["--upload-pack=unexpected".to_owned()])
            .is_err()
    );
}

struct Repository {
    root: tempfile::TempDir,
    workspace: PathBuf,
    remote: PathBuf,
    base: String,
}

impl Repository {
    fn new() -> Self {
        Self::with_format("sha1").expect("SHA-1 Git fixture must initialize")
    }

    fn with_format(format: &str) -> Option<Self> {
        let root = tempfile::tempdir().assert_value();
        let workspace = root.path().join("workspace");
        let remote = root.path().join("remote");
        std::fs::create_dir(&workspace).assert_value();
        let initialization = std::process::Command::new("git")
            .arg("-C")
            .arg(&workspace)
            .args([
                "init",
                "--initial-branch=main",
                &format!("--object-format={format}"),
            ])
            .output()
            .assert_value();
        let detail = String::from_utf8_lossy(&initialization.stderr);
        if !initialization.status.success()
            && format == "sha256"
            && (detail.contains("unknown hash algorithm") || detail.contains("unknown option"))
        {
            eprintln!("installed Git has no SHA-256 support: {detail}");
            return None;
        }
        assert!(
            initialization.status.success(),
            "Git fixture init failed: {detail}"
        );
        git(&workspace, &["config", "user.name", "Test"]);
        git(
            &workspace,
            &["config", "user.email", "test@example.invalid"],
        );
        std::fs::write(workspace.join("base"), "base").assert_value();
        git(&workspace, &["add", "--all"]);
        git(&workspace, &["commit", "--message", "base"]);
        let base = git(&workspace, &["rev-parse", "HEAD"]);
        git(
            &workspace,
            &[
                "clone",
                "--bare",
                workspace.to_str().assert_value(),
                remote.to_str().assert_value(),
            ],
        );
        Some(Self {
            root,
            workspace,
            remote,
            base,
        })
    }

    fn transport(&self, program: &Path, workspace: &Path, remote: &OsStr) -> GitTransport {
        GitTransport::new(
            GitTransportConfig {
                git_program: program.to_owned(),
                workspace: workspace.to_owned(),
                home_directory: self.root.path().to_owned(),
                private_directory: self.root.path().to_owned(),
                identity: None,
                local_environment: None,
                deadline: GIT_FIXTURE_BUDGET,
            },
            remote,
        )
    }

    fn native_transport(
        &self,
        remote: &OsStr,
        configuration: &Path,
        variables: BTreeMap<String, String>,
    ) -> GitTransport {
        let mut config = self
            .transport(Path::new("git"), &self.workspace, remote)
            .config()
            .clone();
        let mut environment = BTreeMap::from([
            ("HOME".to_owned(), self.root.path().display().to_string()),
            ("PATH".to_owned(), std::env::var("PATH").assert_value()),
            (
                "GIT_CONFIG_GLOBAL".to_owned(),
                configuration.display().to_string(),
            ),
        ]);
        environment.extend(variables);
        config.local_environment = Some(environment);
        GitTransport::new(config, remote)
    }

    async fn read_https_ref(
        &self,
        remote: &str,
        username: &str,
        password: &str,
    ) -> Result<Option<String>, SourceDeliveryError> {
        let credential = SourceDeliveryCredential::token("")
            .with_git(SourceGitCredential::Https { username, password });
        self.transport(Path::new("git"), &self.workspace, OsStr::new(remote))
            .read_ref("main", credential)
            .await
    }

    fn commit(&self, name: &str) -> String {
        std::fs::write(self.workspace.join(name), name).assert_value();
        git(&self.workspace, &["add", "--all"]);
        git(&self.workspace, &["commit", "--message", name]);
        git(&self.workspace, &["rev-parse", "HEAD"])
    }
}

fn git(workspace: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "Git fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .assert_value()
        .trim()
        .to_owned()
}

#[tokio::test]
async fn exact_checkout_and_captured_candidate_push_use_git_alone() {
    let repository = Repository::new();
    let admitted = repository.base.clone();
    repository.commit("source-advanced");
    git(
        &repository.workspace,
        &["push", repository.remote.to_str().assert_value(), "main"],
    );
    let checkout = repository.root.path().join("checkout");
    std::fs::create_dir(&checkout).assert_value();
    let git_program = Path::new("git");
    let transport = repository.transport(git_program, &checkout, repository.remote.as_os_str());
    let credential = SourceDeliveryCredential::token("");
    transport
        .checkout("main", &admitted, credential)
        .await
        .assert_value();
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), admitted);
    assert!(!checkout.join("source-advanced").exists());
    let candidate = repository.commit("candidate");
    let later = repository.commit("later");
    assert_ne!(candidate, later);
    let transport = repository.transport(
        git_program,
        &repository.workspace,
        repository.remote.as_os_str(),
    );
    transport
        .push("zeroshot/test", &candidate, credential)
        .await
        .assert_value();
    assert_eq!(
        transport
            .read_ref("zeroshot/test", credential)
            .await
            .assert_value(),
        Some(candidate)
    );
    assert_eq!(git(&repository.workspace, &["rev-parse", "HEAD"]), later);
}

#[cfg(unix)]
#[tokio::test]
async fn an_accepted_push_with_a_lost_response_is_confirmed_through_git() {
    let repository = Repository::new();
    let revision = repository.commit("candidate");
    let program = repository.root.path().join("lost-push");
    let source = r#"#!/bin/sh
for argument in "$@"; do
    if test "$argument" = push; then
        /usr/bin/git "$@"
        exit 77
    fi
done
exec /usr/bin/git "$@"
"#;
    openengine_cluster_testkit::fixture::write_executable(&program, source, 0o700).assert_value();
    let transport = repository.transport(
        &program,
        &repository.workspace,
        repository.remote.as_os_str(),
    );
    transport
        .push(
            "zeroshot/lost",
            &revision,
            SourceDeliveryCredential::token(""),
        )
        .await
        .assert_value();
    assert_eq!(
        git(
            &repository.remote,
            &["rev-parse", "refs/heads/zeroshot/lost"]
        ),
        revision
    );
}

#[cfg(unix)]
#[tokio::test]
async fn https_authority_is_private_and_diagnostics_redact_raw_and_encoded_passwords() {
    let repository = Repository::new();
    let program = repository.root.path().join("print-authentication");
    let source = r#"#!/bin/sh
printf '%s\n' "$GIT_CONFIG_VALUE_1"
printf '%s\n' 'private-test-password' >&2
exit 17
"#;
    openengine_cluster_testkit::fixture::write_executable(&program, source, 0o700).assert_value();
    let transport = repository.transport(
        &program,
        &repository.workspace,
        OsStr::new("https://forge.invalid/team/nested/project.git"),
    );
    let credential = SourceDeliveryCredential::token("").with_git(SourceGitCredential::Https {
        username: "git",
        password: "private-test-password",
    });
    let error = transport
        .read_ref("main", credential)
        .await
        .expect_err("fixture must fail")
        .to_string();
    assert!(error.contains("[REDACTED]"));
    assert!(!error.contains("private-test-password"));
    assert!(
        !error.contains(&super::super::git_auth::encode_user_password(
            "git",
            "private-test-password"
        ))
    );
    assert!(!format!("{credential:?}").contains("private-test-password"));
    assert!(!git(&repository.workspace, &["config", "--list"]).contains("private-test-password"));
}

#[test]
fn git_ref_evidence_accepts_sha256_and_rejects_wrong_or_duplicate_refs() {
    let revision = "a".repeat(64);
    let reference = "refs/heads/main";
    assert_eq!(
        parse_ref(&format!("{revision}\t{reference}\n"), reference).assert_value(),
        Some(revision.clone())
    );
    for output in [
        format!("{revision}\trefs/heads/other\n"),
        format!("{revision}\t{reference}\n{revision}\t{reference}\n"),
        "bad ref".to_owned(),
    ] {
        assert!(parse_ref(&output, reference).is_err());
    }
}

#[tokio::test]
async fn sha256_source_checkout_and_exact_candidate_push_use_the_admitted_object_format() {
    let Some(repository) = Repository::with_format("sha256") else {
        return;
    };
    assert_eq!(repository.base.len(), 64);
    let candidate = repository.commit("sha256-candidate");
    let checkout = repository.root.path().join("sha256-checkout");
    std::fs::create_dir(&checkout).assert_value();
    let credential = SourceDeliveryCredential::token("");
    let transport =
        repository.transport(Path::new("git"), &checkout, repository.remote.as_os_str());
    transport
        .checkout("main", &repository.base, credential)
        .await
        .assert_value();
    assert_eq!(
        git(&checkout, &["rev-parse", "--show-object-format"]),
        "sha256"
    );
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), repository.base);
    repository
        .transport(
            Path::new("git"),
            &repository.workspace,
            repository.remote.as_os_str(),
        )
        .push("zeroshot/sha256", &candidate, credential)
        .await
        .assert_value();
    assert_eq!(
        git(
            &repository.remote,
            &["rev-parse", "refs/heads/zeroshot/sha256"]
        ),
        candidate
    );
}

#[cfg(unix)]
#[tokio::test]
async fn forge_tokens_and_legacy_github_tokens_cannot_authorize_unrelated_git_remotes() {
    let repository = Repository::new();
    let program = repository.root.path().join("credential-scope-spy");
    openengine_cluster_testkit::fixture::write_executable(
        &program,
        "#!/bin/sh\ntest -z \"$GH_TOKEN$GIT_CONFIG_COUNT$GIT_SSH_COMMAND\" || exit 89\n",
        0o700,
    )
    .assert_value();
    let legacy = source_credential("legacy-github-secret", None);
    let forge = SourceDeliveryCredential::token("unrelated-forge-secret");
    for (remote, credential) in [
        ("https://forge.invalid/nested/repo.git", legacy),
        ("git@forge.invalid:nested/repo.git", legacy),
        ("https://github.com/acme/project.git", forge),
    ] {
        let transport = repository.transport(&program, &repository.workspace, OsStr::new(remote));
        assert_eq!(
            transport.read_ref("main", credential).await.assert_value(),
            None
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn native_repository_redirects_cannot_replace_the_selected_workspace() {
    let repository = Repository::new();
    let unrelated = repository.root.path().join("unrelated");
    std::fs::create_dir(&unrelated).assert_value();
    git(&unrelated, &["init", "--initial-branch=main"]);
    let mut config = repository
        .transport(
            Path::new("git"),
            &repository.workspace,
            repository.remote.as_os_str(),
        )
        .config()
        .clone();
    config.local_environment = Some(BTreeMap::from([
        (
            "GIT_DIR".to_owned(),
            unrelated.join(".git").display().to_string(),
        ),
        ("GIT_WORK_TREE".to_owned(), unrelated.display().to_string()),
    ]));
    let transport = GitTransport::new(config, repository.remote.as_os_str());
    let observed = transport
        .success(&["rev-parse", "HEAD"], SourceDeliveryCredential::token(""))
        .await
        .assert_value();
    assert_eq!(observed.stdout().trim(), repository.base);
}

fn http_response_fixture(response: Vec<u8>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").assert_value();
    listener.set_nonblocking(true).assert_value();
    let remote = format!(
        "http://{}/project.git",
        listener.local_addr().assert_value()
    );
    let server = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let deadline = started + GIT_FIXTURE_BUDGET;
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(std::time::Instant::now() < deadline, "Git did not connect");
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(error) => panic!("HTTP credential fixture could not accept Git: {error}"),
            }
        };
        // Windows sockets inherit the listener's nonblocking mode.
        stream.set_nonblocking(false).assert_value();
        let mut request = Vec::new();
        while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            stream
                .set_read_timeout(Some(http_remaining(deadline)))
                .expect("HTTP fixture read timeout");
            let mut buffer = [0_u8; 1024];
            let read = stream.read(&mut buffer).unwrap_or_else(|error| {
                panic!(
                    "HTTP fixture read failed after {:?}, {} bytes ({:?}): {error}",
                    started.elapsed(),
                    request.len(),
                    error.kind()
                )
            });
            assert_ne!(read, 0, "Git closed an incomplete request");
            request.extend_from_slice(&buffer[..read]);
            assert!(request.len() <= 64 * 1024, "Git request exceeded its bound");
        }
        let request = String::from_utf8(request).assert_value();
        let authorization = request
            .lines()
            .filter_map(|line| line.split_once(':'))
            .filter(|(name, _)| name.eq_ignore_ascii_case("authorization"))
            .map(|(_, value)| value.trim().to_owned())
            .collect::<Vec<_>>();
        let mut response = response.as_slice();
        while !response.is_empty() {
            stream
                .set_write_timeout(Some(http_remaining(deadline)))
                .expect("HTTP fixture write timeout");
            let written = stream.write(response).unwrap_or_else(|error| {
                panic!(
                    "HTTP fixture response failed after {:?}: {error}",
                    started.elapsed()
                )
            });
            assert_ne!(written, 0, "HTTP fixture response write returned zero");
            response = &response[written..];
        }
        authorization
    });
    (remote, server)
}

fn http_remaining(deadline: std::time::Instant) -> Duration {
    deadline
        .checked_duration_since(std::time::Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .expect("HTTP fixture deadline expired")
}

fn http_authorization(
    server: std::thread::JoinHandle<Vec<String>>,
    result: &Result<Option<String>, SourceDeliveryError>,
) -> Vec<String> {
    server
        .join()
        .unwrap_or_else(|_| panic!("HTTP fixture failed; Git result: {result:?}"))
}

#[tokio::test]
async fn explicit_https_credentials_replace_native_authorization_headers() {
    let repository = Repository::new();
    let (remote, server) = http_response_fixture(
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    );
    let configuration = repository.root.path().join(".gitconfig");
    let native_configuration = format!(
        "[http \"{remote}\"]\n\textraHeader = AUTHORIZATION: basic stale-native-one\n\
         \textraHeader = AUTHORIZATION: basic stale-native-two\n"
    );
    std::fs::write(&configuration, &native_configuration).assert_value();
    let credential = SourceDeliveryCredential::token("").with_git(SourceGitCredential::Https {
        username: "git-user",
        password: "declared-source-password",
    });
    let result = repository
        .native_transport(OsStr::new(&remote), &configuration, BTreeMap::new())
        .read_ref("main", credential)
        .await;
    let authorization = http_authorization(server, &result);
    let failure = result.expect_err("fixture endpoint returns HTTP 404");
    let SourceDeliveryError::Command(diagnostic) = failure else {
        panic!("HTTP 404 must retain the Git command diagnostic: {failure}");
    };
    assert!(!diagnostic.timed_out, "{diagnostic}");
    assert!(!diagnostic.cleanup_unconfirmed, "{diagnostic}");
    assert!(
        diagnostic.exit_status.is_some_and(|code| code != 0),
        "{diagnostic}"
    );
    assert!(
        diagnostic.http_status() == Some(404) || diagnostic.stderr.contains("not found"),
        "Git must observe the fixture's HTTP 404: {diagnostic}"
    );
    assert_eq!(
        authorization,
        vec![format!(
            "basic {}",
            super::super::git_auth::encode_user_password("git-user", "declared-source-password")
        )]
    );
    assert_eq!(
        std::fs::read_to_string(configuration).assert_value(),
        native_configuration,
        "explicit source authority must not rewrite native Git configuration"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn native_reconciliation_redacts_git_filter_secret_output() {
    use crate::source_code_provider::{
        SourceHeadReconciliation, SourceReconciliationOutcome, SourceReviewReceipt,
    };

    let repository = Repository::new();
    std::fs::write(
        repository.workspace.join(".gitattributes"),
        "filtered.txt filter=credential-canary\n",
    )
    .assert_value();
    std::fs::write(repository.workspace.join("filtered.txt"), "base\n").assert_value();
    let published_revision = repository.commit("published-with-filter");
    let local_revision = repository.commit("retained-local-work");
    git(
        &repository.workspace,
        &["checkout", "--detach", &published_revision],
    );
    std::fs::write(repository.workspace.join("filtered.txt"), "upstream\n").assert_value();
    let observed_revision = repository.commit("upstream-filtered-work");
    let branch = "zeroshot/native-credentials";
    git(
        &repository.workspace,
        &[
            "push",
            repository.remote.to_str().assert_value(),
            &format!("{observed_revision}:refs/heads/{branch}"),
        ],
    );
    git(
        &repository.workspace,
        &["checkout", "--detach", &local_revision],
    );
    let configuration = repository.root.path().join(".gitconfig");
    let native_configuration = r#"[filter "credential-canary"]
    smudge = "sh -c 'printf \"%s\\n\" \"$GIT_PASSWORD\" >&2; cat'"
    clean = cat
"#;
    std::fs::write(&configuration, native_configuration).assert_value();
    let secret = "source-native-password-canary-9bf3";
    let transport = repository.native_transport(
        repository.remote.as_os_str(),
        &configuration,
        BTreeMap::from([("GIT_PASSWORD".to_owned(), secret.to_owned())]),
    );
    let published = SourceReviewReceipt {
        review_id: String::new(),
        repository: "git-source:opaque".to_owned(),
        target_branch: "main".to_owned(),
        head_branch: branch.to_owned(),
        head_revision: published_revision,
    };
    let observed = SourceReviewReceipt {
        head_revision: observed_revision,
        ..published.clone()
    };
    let outcome = reconcile_head(
        &transport,
        SourceHeadReconciliation {
            workspace: &repository.workspace,
            published: &published,
            observed: &observed,
            commit_message: "Preserve local work while integrating the observed head",
            authorized_update: false,
            adopting_existing: false,
        },
        SourceDeliveryCredential::token(""),
    )
    .await
    .assert_value();
    let SourceReconciliationOutcome::NeedsWork(diagnostic) = outcome else {
        panic!("merged remote work must return through candidate review: {outcome:?}");
    };
    assert!(
        diagnostic.contains("[REDACTED]"),
        "native Git filter did not run: {diagnostic}"
    );
    assert!(
        !diagnostic.contains(secret),
        "native transport secret entered repair feedback"
    );
    assert!(repository.workspace.join("retained-local-work").is_file());
    assert_eq!(
        std::fs::read_to_string(repository.workspace.join("filtered.txt")).assert_value(),
        "upstream\n"
    );
    assert_eq!(
        std::fs::read_to_string(configuration).assert_value(),
        native_configuration,
        "reconciliation must not rewrite native Git configuration"
    );
}

#[tokio::test]
async fn short_https_username_cannot_corrupt_successful_git_ref_evidence() {
    let repository = Repository::new();
    let revision = "ab".repeat(20);
    let record = format!("{revision} refs/heads/main\0agent=git/fixture\n");
    let advertisement = format!(
        "001e# service=git-upload-pack\n0000{:04x}{record}0000",
        record.len() + 4
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-git-upload-pack-advertisement\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{advertisement}",
        advertisement.len()
    );
    let (remote, server) = http_response_fixture(response.into_bytes());
    let result = repository
        .read_https_ref(&remote, "a", "private-ref-password")
        .await;
    let authorization = http_authorization(server, &result);
    assert_eq!(result.assert_value(), Some(revision));
    assert_eq!(
        authorization,
        vec![format!(
            "basic {}",
            super::super::git_auth::encode_user_password("a", "private-ref-password")
        )]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn checkout_revision_mismatch_prints_only_sanitized_protocol_output() {
    let repository = Repository::new();
    let program = repository.root.path().join("checkout-protocol-canary");
    let private_username = "private-protocol-username";
    let private_password = "private-protocol-password";
    let source = r#"#!/bin/sh
for argument in "$@"; do
    if test "$argument" = rev-parse; then
        printf '%s %s\n' 'private-protocol-username' 'private-protocol-password'
    fi
done
exit 0
"#;
    openengine_cluster_testkit::fixture::write_executable(&program, source, 0o700).assert_value();
    let credential = SourceDeliveryCredential::token("").with_git(SourceGitCredential::Https {
        username: private_username,
        password: private_password,
    });
    let error = repository
        .transport(
            &program,
            &repository.workspace,
            OsStr::new("https://forge.invalid/project.git"),
        )
        .checkout("main", &repository.base, credential)
        .await
        .expect_err("canary output must not prove an exact checkout");
    let diagnostic = error.to_string();
    assert!(diagnostic.contains("observed [REDACTED] [REDACTED]"));
    assert!(!diagnostic.contains(private_username));
    assert!(!diagnostic.contains(private_password));
}

#[cfg(unix)]
#[tokio::test]
async fn explicit_https_rejection_never_invokes_native_askpass() {
    for control in ["GIT_ASKPASS", "SSH_ASKPASS", "core.askPass"] {
        let repository = Repository::new();
        let helper = repository.root.path().join("native-askpass");
        let sentinel = repository.root.path().join("askpass-was-invoked");
        openengine_cluster_testkit::fixture::write_executable(
            &helper,
            "#!/bin/sh\nprintf 'invoked\\n' >> \"$ASKPASS_SENTINEL\"\nprintf 'native-helper-credential\\n'\n",
            0o700,
        )
        .assert_value();
        let (remote, server) = http_response_fixture(
            b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"source\"\r\n\
              Content-Length: 0\r\nConnection: close\r\n\r\n"
                .to_vec(),
        );
        let configuration = repository.root.path().join(".gitconfig");
        let native_configuration = if control == "core.askPass" {
            format!("[core]\n\taskPass = {}\n", helper.display())
        } else {
            String::new()
        };
        std::fs::write(&configuration, &native_configuration).assert_value();
        let mut variables = BTreeMap::from([(
            "ASKPASS_SENTINEL".to_owned(),
            sentinel.display().to_string(),
        )]);
        if control != "core.askPass" {
            variables.insert(control.to_owned(), helper.display().to_string());
        }
        let credential = SourceDeliveryCredential::token("").with_git(SourceGitCredential::Https {
            username: "a",
            password: "declared-challenge-password",
        });
        let result = repository
            .native_transport(OsStr::new(&remote), &configuration, variables)
            .read_ref("main", credential)
            .await;
        let authorization = http_authorization(server, &result);
        let failure = result.expect_err("fixture rejects the explicit credential");
        assert!(failure.authentication_failed(), "{control}: {failure}");
        assert!(!failure.retryable_operation());
        assert!(!sentinel.exists(), "native {control} helper was invoked");
        assert_eq!(
            authorization,
            vec![format!(
                "basic {}",
                super::super::git_auth::encode_user_password("a", "declared-challenge-password")
            )]
        );
        assert_eq!(
            std::fs::read_to_string(configuration).assert_value(),
            native_configuration,
            "explicit authority must preserve the caller configuration"
        );
    }
}

#[tokio::test]
async fn redacted_http_status_preserves_refusal_and_retry_routing() {
    for status in [401, 429, 503] {
        let repository = Repository::new();
        let (remote, server) = http_response_fixture(
            format!(
                "HTTP/1.1 {status} Fixture Failure\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .into_bytes(),
        );
        let password = status.to_string();
        let result = repository.read_https_ref(&remote, "a", &password).await;
        let authorization = http_authorization(server, &result);
        let failure = result.expect_err("fixture returns a classified HTTP failure");
        assert_eq!(failure.authentication_failed(), status == 401, "{failure}");
        assert_eq!(failure.retryable_operation(), status != 401, "{failure}");
        let SourceDeliveryError::Command(diagnostic) = failure else {
            panic!("HTTP failure must retain the Git command diagnostic");
        };
        // Git translates HTTP 401 into its credential-prompt failure instead of a numeric error.
        assert_eq!(diagnostic.http_status(), (status != 401).then_some(status));
        assert!(!diagnostic.stderr.contains(&password));
        assert!(diagnostic.stderr.contains("[REDACTED]"));
        assert_eq!(
            authorization,
            vec![format!(
                "basic {}",
                super::super::git_auth::encode_user_password("a", &password)
            )]
        );
    }
}

#[tokio::test]
async fn runtime_commit_identity_is_pinned_after_native_environment() {
    let repository = Repository::new();
    let configuration = repository.root.path().join("caller.gitconfig");
    std::fs::write(&configuration, "").assert_value();
    let environment = BTreeMap::from([
        ("GIT_AUTHOR_NAME".to_owned(), "Caller Author".to_owned()),
        (
            "GIT_AUTHOR_EMAIL".to_owned(),
            "author@example.invalid".to_owned(),
        ),
        (
            "GIT_COMMITTER_NAME".to_owned(),
            "Caller Committer".to_owned(),
        ),
        (
            "GIT_COMMITTER_EMAIL".to_owned(),
            "committer@example.invalid".to_owned(),
        ),
    ]);
    let transport =
        repository.native_transport(repository.remote.as_os_str(), &configuration, environment);
    transport
        .execute(
            &[
                "commit".into(),
                "--allow-empty".into(),
                "--message".into(),
                "fixed identity".into(),
            ],
            SourceDeliveryCredential::token(""),
            GIT_FIXTURE_BUDGET,
        )
        .await
        .assert_value()
        .require_success()
        .assert_value();
    assert_eq!(
        git(
            &repository.workspace,
            &["show", "-s", "--format=%an <%ae>|%cn <%ce>"]
        ),
        "Zeroshot <delivery@zeroshot.invalid>|Zeroshot <delivery@zeroshot.invalid>"
    );
}
