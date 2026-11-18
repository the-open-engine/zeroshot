#![cfg(target_os = "linux")]

use super::*;
use crate::execution::process::HostedProcessScope;

const CONTEXT_URL: &str = "https://github.com/acme/library.git";

async fn fixture_with_context() -> (CheckoutFixture, RepositoryFixture) {
    let mut fixture = CheckoutFixture::new("").await;
    let mut context = RepositoryFixture::new();
    // Give this remote a commit the primary source cannot supply.
    let tree = git_output(&context.remote, &["rev-parse", "feature^{tree}"]);
    context.feature_revision = git_output(
        &context.remote,
        &[
            "-c",
            "user.name=Context",
            "-c",
            "user.email=context@example.invalid",
            "commit-tree",
            &tree,
            "-p",
            &context.feature_revision,
            "-m",
            "Context-only commit",
        ],
    );
    context.move_feature_to(&context.feature_revision);
    // Local upload-pack drops command-line safe.directory settings. A protected global config
    // lets distinct run UIDs share these bare test remotes, as an HTTPS server would allow.
    let git_config = fixture.root.write(
        "git-safe-config",
        &format!(
            "[safe]\n\tdirectory = {}\n\tdirectory = {}\n",
            context.remote.display(),
            fixture._repository.remote.display(),
        ),
    );
    // Rewrites transport only: the checkout retains its credential-free HTTPS origin.
    fixture.root.write_executable("git-script", &format!(
        "#!/bin/sh\nexport GIT_CONFIG_GLOBAL='{}'\nexec /usr/bin/git -c 'url.{}.insteadOf={CONTEXT_URL}' \"$@\"\n",
        git_config.display(), context.remote.display(),
    ));
    fixture.admitted.context_sources =
        vec![serde_json::from_value(json!({
        "repository":"acme/library", "branch":"feature", "revision":context.feature_revision,
        "remote":{"url":CONTEXT_URL}
    })).assert_value()];
    fixture.admitted.environment = Some(serde_json::from_value(json!({
        "contextRepositories":[{"repository":"acme/library","branch":"feature"}],
        "startup":"git -C \"$ZEROSHOT_CONTEXT/acme/library\" rev-parse HEAD > startup-context-head"
    })).assert_value());
    (fixture, context)
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn root_context_checkout_has_full_history_and_reclones_the_pin_on_checkpoint_restore() {
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let (fixture, context) = fixture_with_context().await;
    let capsule = fixture.allocate().await.assert_value();
    let run_path = fixture.allocator.run_path(&fixture.run_id);
    let checkout = run_path.join("context/acme/library");
    assert_eq!(
        git_output(&checkout, &["rev-parse", "HEAD"]),
        context.feature_revision
    );
    assert_eq!(git_output(&checkout, &["rev-list", "--count", "HEAD"]), "3");
    assert!(!checkout.join(".git/shallow").exists());
    assert_eq!(
        fs::read_to_string(run_path.join("workspace/startup-context-head"))
            .assert_value()
            .trim(),
        context.feature_revision
    );
    // Delivery's real staging operation is confined to the workspace.
    let workspace = run_path.join("workspace");
    git(&workspace, &["add", "--all"]);
    assert_eq!(
        git_output(&workspace, &["diff", "--cached", "--name-only"]),
        "startup-context-head"
    );
    git(
        &workspace,
        &[
            "-c",
            "user.name=Context",
            "-c",
            "user.email=context@example.invalid",
            "commit",
            "-m",
            "workspace only",
        ],
    );
    assert_eq!(
        git_output(&workspace, &["ls-tree", "--name-only", "HEAD"]),
        "README.md\nstartup-context-head"
    );
    capture_checkpoint(&capsule).await;
    let page = fixture.checkpoints(&fixture.run_id).await;
    let checkpoint_id = page.checkpoints[0].checkpoint_id.clone();
    finish_checkpoints(&capsule).await;
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Failed)
        .await
        .assert_value();
    assert!(!run_path.join("context").exists());
    context.move_feature_to(&context.main_revision);
    let successor = RunId::new("context-checkpoint-successor");
    let mut request = fixture.retained_request(
        &fixture.run_id,
        &successor,
        CheckpointRestoreSelection::Checkpoint { checkpoint_id },
    );
    request.github_token = Some("fresh-source-credential");
    let resumed = fixture
        .allocator
        .allocate_from_retained(request)
        .await
        .assert_value();
    let restored = fixture.allocator.run_path(&successor);
    assert_eq!(
        git_output(
            &restored.join("context/acme/library"),
            &["rev-parse", "HEAD"]
        ),
        context.feature_revision
    );
    assert_eq!(
        fs::read_dir(restored.join("workspace"))
            .assert_value()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| name == "context" || name == "library")
            .count(),
        0
    );
    resumed
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Completed)
        .await
        .assert_value();
}

#[tokio::test]
async fn context_checkout_failure_names_the_repository_and_prevents_startup() {
    for inaccessible in [false, true] {
        let (mut fixture, _context) = fixture_with_context().await;
        if inaccessible {
            fixture.root.write_executable("git-script", "#!/bin/sh\ncase \" $* \" in *acme/library*) exit 42;; esac\nexec /usr/bin/git \"$@\"\n");
        } else {
            fixture.admitted.context_sources[0].revision =
                openengine_cluster_protocol::SourceRevisionId::new("f".repeat(40)).assert_value();
        }
        assert!(matches!(
            fixture.allocate().await,
            Err(CapsuleAllocationUnavailable::ContextCheckout)
        ));
        let diagnostics = fixture.diagnostics.snapshot(&fixture.run_id);
        let encoded = serde_json::to_string(&diagnostics).assert_value();
        assert!(encoded.contains("acme/library"), "{encoded}");
        assert!(!encoded.contains(CHECKOUT_TOKEN));
        let workspace = fixture
            .allocator
            .run_path(&fixture.run_id)
            .join("workspace");
        assert!(!workspace.join("startup-context-head").exists());
    }
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn root_context_checkout_is_read_only_and_experimental_copies_have_no_credentials() {
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let (fixture, context_repository) = fixture_with_context().await;
    let capsule = fixture.allocate().await.assert_value();
    let root = fixture.allocator.run_path(&fixture.run_id);
    let context = root.join("context");
    fn scan(path: &Path) {
        for entry in fs::read_dir(path).assert_value() {
            let entry = entry.assert_value();
            if entry.file_type().assert_value().is_dir() {
                scan(&entry.path());
            } else if entry.file_type().assert_value().is_file() {
                let bytes = fs::read(entry.path()).assert_value();
                assert!(
                    !bytes
                        .windows(CHECKOUT_TOKEN.len())
                        .any(|window| window == CHECKOUT_TOKEN.as_bytes())
                );
            }
        }
    }
    scan(&context);
    let scratch = root.join("experiment-scratch");
    fs::create_dir(&scratch).assert_value();
    // The portable workspace fixture is supervisor-owned, so ask the live allocation for its UID.
    let writer = fixture
        .allocator
        .allocated_process_pool(&fixture.run_id)
        .await
        .assert_value()
        .identity(HostedProcessScope::Writer)
        .assert_value();
    assert_ne!(writer.uid(), unsafe { libc::geteuid() });
    assert_ne!(
        writer.uid(),
        fs::metadata(root.join("workspace")).assert_value().uid()
    );
    std::os::unix::fs::chown(&scratch, Some(writer.uid()), Some(writer.gid())).assert_value();

    // A real concurrent allocation shares the primary GID but must not see this private history.
    let peer_id = RunId::new("context-concurrent-peer");
    let mut peer_admitted = fixture.admitted.clone();
    peer_admitted.context_sources.clear();
    peer_admitted.environment = None;
    let peer = fixture
        .allocator
        .allocate(
            crate::native_v2_candidate::test_support::allocation_request(
                &peer_id,
                &peer_admitted,
                Some(CHECKOUT_TOKEN),
            ),
        )
        .await;
    let peer = peer.unwrap_or_else(|error| {
        panic!(
            "peer allocation failed: {error:?}; diagnostics: {:?}",
            fixture.diagnostics.snapshot(&peer_id)
        )
    });
    let peer_writer = fixture
        .allocator
        .allocated_process_pool(&peer_id)
        .await
        .assert_value()
        .identity(HostedProcessScope::Writer)
        .assert_value();
    assert_ne!(writer.uid(), peer_writer.uid());
    assert_eq!(writer.gid(), peer_writer.gid());
    let denied = Command::new("/bin/sh")
        .args([
            "-c",
            r#"set -eu
test -x "$RUN_ROOT"
if ls "$ZEROSHOT_CONTEXT"; then exit 21; fi
if cat "$ZEROSHOT_CONTEXT/acme/library/README.md"; then exit 22; fi
if cat "$ZEROSHOT_CONTEXT/acme/library/.git/HEAD"; then exit 23; fi
"#,
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("RUN_ROOT", &root)
        .env("ZEROSHOT_CONTEXT", &context)
        .uid(peer_writer.uid())
        .gid(peer_writer.gid())
        .output()
        .assert_value();
    assert!(
        denied.status.success(),
        "{}",
        String::from_utf8_lossy(&denied.stderr)
    );
    fn own_remote(path: &Path) {
        let metadata = fs::symlink_metadata(path).assert_value();
        if metadata.is_dir() {
            for entry in fs::read_dir(path).assert_value() {
                own_remote(&entry.assert_value().path());
            }
        }
        std::os::unix::fs::chown(path, Some(0), Some(0)).assert_value();
    }
    own_remote(&context_repository.remote);
    let script = r#"set -eu
original="$ZEROSHOT_CONTEXT/acme/library"
if printf bad > "$original/README.md"; then exit 11; fi
if chmod u+w "$original/README.md"; then exit 12; fi
if touch "$original/new-file"; then exit 13; fi
if rm "$original/README.md"; then exit 14; fi
if touch "$ZEROSHOT_CONTEXT/replacement"; then exit 15; fi
test "$(git -C "$original" rev-list --count HEAD)" = 3
cp -R "$original" "$TMPDIR/library"
chmod -R u+w "$TMPDIR/library"
printf experiment >> "$TMPDIR/library/README.md"
test "$(git -C "$TMPDIR/library" diff --name-only)" = README.md
test "$(git -C "$TMPDIR/library" rev-list --count HEAD)" = 3
# The origin is credential-free, and the copied checkout has no credential helper.
test "$(git -C "$TMPDIR/library" remote get-url origin)" = https://github.com/acme/library.git
test -z "$(git -C "$TMPDIR/library" config --get-all credential.helper || true)"
if git -C "$TMPDIR/library" -c "url.$CONTEXT_PUSH_REMOTE.insteadOf=https://github.com/acme/library.git" -c "safe.directory=$CONTEXT_PUSH_REMOTE" push origin HEAD:refs/heads/experiment; then exit 16; fi
test -z "${GH_TOKEN-}${GITHUB_TOKEN-}${GIT_PASSWORD-}${GIT_SSH_PRIVATE_KEY-}"
"#;
    let output = Command::new("/bin/sh")
        .args(["-c", script])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", &scratch)
        .env("TMPDIR", &scratch)
        .env("CONTEXT_PUSH_REMOTE", &context_repository.remote)
        .env("ZEROSHOT_CONTEXT", &context)
        .env("GIT_CONFIG_GLOBAL", root.join("runtime/context-gitconfig"))
        .uid(writer.uid())
        .gid(writer.gid())
        .output()
        .assert_value();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        git_output(
            &context_repository.remote,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads/experiment"
            ]
        )
        .is_empty()
    );
    fs::remove_dir_all(&scratch).assert_value();
    peer.cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Completed)
        .await
        .assert_value();
    capsule
        .cleanup
        .destroy_or_confirm_absent(RunRuntimeExit::Completed)
        .await
        .assert_value();
}
