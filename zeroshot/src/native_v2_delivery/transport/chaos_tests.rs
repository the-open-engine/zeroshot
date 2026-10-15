//! Hostile trusted-Git fixture output must not escape private transport authority.
#![cfg(unix)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::Duration,
};

use super::{GitTransport, GitTransportConfig};
use crate::source_code_provider::{SourceDeliveryCredential, SourceGitCredential};

type TempRepo = crate::native_v2_candidate::test_support::TestGitRepository;
use crate::native_v2_delivery::transport_http as http_fixture;

struct ScriptTransport {
    root: tempfile::TempDir,
    transport: GitTransport,
}

impl ScriptTransport {
    fn new(script: &str, remote: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let program = root.path().join("git-fixture");
        openengine_cluster_testkit::fixture::write_executable(&program, script, 0o700).unwrap();
        let transport = GitTransport::new(
            GitTransportConfig {
                git_program: program,
                workspace,
                home_directory: root.path().to_owned(),
                private_directory: root.path().to_owned(),
                identity: None,
                local_environment: None,
                deadline: Duration::from_secs(2),
            },
            remote,
        );
        Self { root, transport }
    }

    fn with_native_environment(mut self, environment: BTreeMap<String, String>) -> Self {
        let mut config = self.transport.config().clone();
        config.local_environment = Some(environment);
        self.transport = GitTransport::new(config, self.transport.remote());
        self
    }

    fn private_auth_directories(&self) -> Vec<PathBuf> {
        fs::read_dir(self.root.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("git-auth-")
            })
            .collect()
    }
}

#[tokio::test]
async fn ssh_helper_line_echo_cannot_retain_a_private_key_fragment() {
    let fixture = ScriptTransport::new(
        r#"#!/bin/sh
eval "set -- $GIT_SSH_COMMAND"
previous=''
for argument in "$@"; do
    if test "$previous" = '-i'; then
        /usr/bin/sed -n '2p' "$argument" >&2
    fi
    previous="$argument"
done
exit 29
"#,
        "deployer@forge.invalid:team/nested/project.git",
    );
    let private_key = concat!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n",
        "KEY_BODY_MUST_STAY_PRIVATE_chaos_9bf3\n",
        "-----END OPENSSH PRIVATE KEY-----\n",
    );
    let credential = SourceDeliveryCredential::token("").with_git(SourceGitCredential::Ssh {
        private_key,
        known_hosts: "forge.invalid ssh-ed25519 fixture-host-key\n",
    });
    let error = fixture
        .transport
        .read_ref("main", credential)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        !error.contains("KEY_BODY_MUST_STAY_PRIVATE_chaos_9bf3"),
        "private key fragment leaked: {error}"
    );
    assert!(
        fixture.private_auth_directories().is_empty(),
        "private SSH material survived the operation"
    );
}

#[tokio::test]
async fn contained_https_authority_cannot_inherit_a_native_credential_helper() {
    let fixture = ScriptTransport::new(
        r#"#!/bin/sh
if test -n "$GH_TOKEN$GITHUB_TOKEN$SSH_AUTH_SOCK$GIT_ASKPASS"; then
    printf 'ambient-authority-crossed' >&2
    exit 91
fi
if test "$GIT_CONFIG_KEY_0" != credential.helper || test -n "$GIT_CONFIG_VALUE_0"; then
    printf 'helper-not-disabled' >&2
    exit 92
fi
exit 0
"#,
        "https://forge.invalid/team/nested/project.git",
    );
    let credential = SourceDeliveryCredential::token("").with_git(SourceGitCredential::Https {
        username: "git-user",
        password: "contained-password-canary",
    });
    assert_eq!(
        fixture
            .transport
            .read_ref("main", credential)
            .await
            .unwrap(),
        None
    );
    assert!(fixture.private_auth_directories().is_empty());
}

#[tokio::test]
async fn native_git_environment_is_private_and_noninteractive() {
    let fixture = ScriptTransport::new(
        r#"#!/bin/sh
test "$SSH_AUTH_SOCK" = /fixture/agent || exit 81
test "$GIT_ASKPASS" = /fixture/native-helper || exit 82
test "$GIT_TERMINAL_PROMPT" = 0 || exit 83
test "$GCM_INTERACTIVE" = never || exit 84
test "$SSH_ASKPASS_REQUIRE" = never || exit 85
printf '%s' "$GITHUB_TOKEN" >&2
exit 19
"#,
        "https://forge.invalid/team/nested/project.git",
    )
    .with_native_environment(BTreeMap::from([
        ("SSH_AUTH_SOCK".to_owned(), "/fixture/agent".to_owned()),
        (
            "GIT_ASKPASS".to_owned(),
            "/fixture/native-helper".to_owned(),
        ),
        (
            "GITHUB_TOKEN".to_owned(),
            "native-git-canary-453a".to_owned(),
        ),
        ("GIT_TERMINAL_PROMPT".to_owned(), "1".to_owned()),
        ("GCM_INTERACTIVE".to_owned(), "always".to_owned()),
        ("SSH_ASKPASS_REQUIRE".to_owned(), "force".to_owned()),
    ]));
    let error = fixture
        .transport
        .read_ref("main", SourceDeliveryCredential::token(""))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("[REDACTED]"),
        "native authentication was not used: {error}"
    );
    assert!(!error.contains("native-git-canary-453a"));
    assert!(!format!("{:?}", fixture.transport).contains("native-git-canary-453a"));
}

#[tokio::test]
async fn ssh_authentication_replaces_native_agent_and_is_removed_after_failure() {
    let fixture = ScriptTransport::new(
        r#"#!/bin/sh
test -z "$SSH_AUTH_SOCK$SSH_AGENT_PID" || exit 88
case "$GIT_SSH_COMMAND" in
    *StrictHostKeyChecking=yes*IdentityAgent=none*) ;;
    *IdentityAgent=none*StrictHostKeyChecking=yes*) ;;
    *) exit 89 ;;
esac
printf '%s' 'explicit-private-ssh-key-canary' >&2
exit 16
"#,
        "ssh://deploy@forge.invalid:2222/team/nested/project.git",
    )
    .with_native_environment(BTreeMap::from([
        (
            "SSH_AUTH_SOCK".to_owned(),
            "/fixture/native-agent".to_owned(),
        ),
        ("SSH_AGENT_PID".to_owned(), "99999".to_owned()),
    ]));
    let credential = SourceDeliveryCredential::token("").with_git(SourceGitCredential::Ssh {
        private_key: "explicit-private-ssh-key-canary",
        known_hosts: "[forge.invalid]:2222 ssh-ed25519 fixture-host-key",
    });
    let error = fixture
        .transport
        .read_ref("main", credential)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("[REDACTED]"),
        "explicit SSH authentication was not installed: {error}"
    );
    assert!(!error.contains("explicit-private-ssh-key-canary"));
    assert!(fixture.private_auth_directories().is_empty());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn git_main_exit_cannot_leave_a_credential_helper_descendant() {
    let fixture = ScriptTransport::new(
        r#"#!/bin/sh
previous=''
for argument in "$@"; do
    if test "$previous" = -C; then
        cd -- "$argument" || exit 87
        break
    fi
    previous="$argument"
done
/usr/bin/sleep 120 &
printf '%s' "$!" > helper.pid
exit 0
"#,
        "ssh://git@forge.invalid/team/nested/project.git",
    );
    let result = fixture
        .transport
        .read_ref("main", SourceDeliveryCredential::token(""))
        .await;
    let pid: i32 = fs::read_to_string(fixture.transport.config().workspace.join("helper.pid"))
        .unwrap()
        .parse()
        .unwrap();
    let state = linux_process_state(pid);
    let alive = state.is_some_and(|state| !matches!(state, 'Z' | 'X'));
    if alive {
        // SAFETY: only the recorded fixture child receives cleanup on an assertion failure.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    assert!(
        !alive,
        "Git helper retained live work; state: {state:?}; result: {result:?}"
    );
    assert!(
        result.is_ok(),
        "main exit became a pipe timeout: {result:?}"
    );
}

#[cfg(target_os = "linux")]
fn linux_process_state(pid: i32) -> Option<char> {
    let status = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, fields) = status.rsplit_once(')')?;
    fields.trim_start().chars().next()
}

#[tokio::test]
async fn ssh_credential_for_https_refuses_before_git_effects() {
    let fixture = ScriptTransport::new(
        "#!/bin/sh\nprintf effect > \"$HOME/git-effect\"\nexit 0\n",
        "https://forge.invalid/team/nested/project.git",
    );
    let credential = SourceDeliveryCredential::token("").with_git(SourceGitCredential::Ssh {
        private_key: "explicit-ssh-key-for-wrong-transport",
        known_hosts: "forge.invalid ssh-ed25519 fixture-host-key",
    });
    let result = fixture.transport.read_ref("main", credential).await;
    assert!(
        result.is_err(),
        "SSH authority was silently ignored for HTTPS: {result:?}"
    );
    assert!(!fixture.root.path().join("git-effect").exists());
    assert!(fixture.private_auth_directories().is_empty());
}

#[tokio::test]
async fn cancelled_git_cleans_up_descendants_and_private_ssh_material() {
    let fixture = ScriptTransport::new(
        "#!/bin/sh\nprintf '%s' \"$$\" > \"$HOME/git-pid\"\n/usr/bin/sleep 120\n",
        "ssh://deploy@forge.invalid/team/project.git",
    );
    let transport = fixture.transport.clone();
    let pending = tokio::spawn(async move {
        let credential = SourceDeliveryCredential::token("").with_git(SourceGitCredential::Ssh {
            private_key: "cancellation-ssh-secret-canary",
            known_hosts: "forge.invalid ssh-ed25519 fixture-host-key",
        });
        transport.read_ref("main", credential).await
    });
    let marker = fixture.root.path().join("git-pid");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while !marker.exists() {
        assert!(tokio::time::Instant::now() < deadline, "Git never started");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(fixture.private_auth_directories().len(), 1);
    let pid = fs::read_to_string(marker).unwrap().parse::<i32>().unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    // SAFETY: kill(pid, 0) only observes the fixture-owned Git process.
    while unsafe { libc::kill(pid, 0) == 0 } {
        assert!(
            tokio::time::Instant::now() < deadline,
            "cancelled Git survived"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(fixture.private_auth_directories().is_empty());
}

fn git(workspace: &Path, arguments: &[&str]) -> String {
    let result = std::process::Command::new("git")
        .current_dir(workspace)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "fixture Git failed: {arguments:?}: {result:?}"
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn http_credentials() -> SourceDeliveryCredential<'static> {
    SourceDeliveryCredential::token("").with_git(SourceGitCredential::Https {
        username: "x-access-token",
        password: "test-token",
    })
}

fn http_transport(repository: &TempRepo, server: &http_fixture::HttpGit) -> GitTransport {
    let executable = server.git_program(repository);
    repository
        .root
        .write_executable("gh", "#!/bin/sh\nprintf called > gh.called\nexit 96\n");
    GitTransport::new(
        GitTransportConfig {
            git_program: executable,
            workspace: repository.workspace.clone(),
            home_directory: repository.root.path().to_owned(),
            private_directory: repository.root.path().to_owned(),
            identity: None,
            local_environment: Some(BTreeMap::from([
                (
                    "PATH".to_owned(),
                    format!("{}:/usr/bin:/bin", repository.root.path().display()),
                ),
                (
                    "HOME".to_owned(),
                    repository.root.path().to_string_lossy().into_owned(),
                ),
            ])),
            deadline: Duration::from_secs(5),
        },
        &server.url,
    )
}

#[tokio::test]
async fn generic_http_lost_push_response_confirms_reviewed_sha_without_any_forge_call() {
    let repository = TempRepo::delivery();
    fs::write(repository.workspace.join("reviewed"), "reviewed candidate").unwrap();
    crate::native_v2_candidate::test_support::commit_all(&repository.workspace, "reviewed");
    let reviewed = git(&repository.workspace, &["rev-parse", "HEAD"]);
    fs::write(repository.workspace.join("later"), "later local work").unwrap();
    crate::native_v2_candidate::test_support::commit_all(&repository.workspace, "later");
    let later = git(&repository.workspace, &["rev-parse", "HEAD"]);
    let server = http_fixture::HttpGit::start(&repository.remote);
    server
        .faults
        .drop_push_response_once
        .store(true, Ordering::SeqCst);
    let transport = http_transport(&repository, &server);

    transport
        .push("zeroshot/generic-chaos", &reviewed, http_credentials())
        .await
        .unwrap();

    assert_eq!(
        git(
            &repository.remote,
            &["rev-parse", "refs/heads/zeroshot/generic-chaos"]
        ),
        reviewed
    );
    assert_eq!(git(&repository.workspace, &["rev-parse", "HEAD"]), later);
    assert_eq!(server.faults.receive_packs.load(Ordering::SeqCst), 1);
    assert_eq!(server.faults.dropped_responses.load(Ordering::SeqCst), 1);
    assert!(!repository.workspace.join("gh.called").exists());
    assert!(!repository.root.child("gh.called").exists());
    assert!(
        fs::read_to_string(repository.workspace.join("later"))
            .unwrap()
            .contains("later local work")
    );
}

#[tokio::test]
async fn generic_http_authentication_rejection_cannot_create_remote_branch() {
    let repository = TempRepo::delivery();
    let candidate = git(&repository.workspace, &["rev-parse", "HEAD"]);
    let server = http_fixture::HttpGit::start(&repository.remote);
    server
        .faults
        .reject_all_credentials
        .store(true, Ordering::SeqCst);
    let transport = http_transport(&repository, &server);
    let error = transport
        .push("zeroshot/rejected-chaos", &candidate, http_credentials())
        .await
        .unwrap_err();
    assert!(
        error.authentication_failed(),
        "credential rejection was misclassified: {error}"
    );
    assert_eq!(server.faults.receive_packs.load(Ordering::SeqCst), 0);
    assert!(server.faults.denied.load(Ordering::SeqCst) > 0);
    let result = std::process::Command::new("git")
        .arg("--git-dir")
        .arg(&repository.remote)
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            "refs/heads/zeroshot/rejected-chaos",
        ])
        .status()
        .unwrap();
    assert!(!result.success());
    assert!(!repository.workspace.join("gh.called").exists());
}
