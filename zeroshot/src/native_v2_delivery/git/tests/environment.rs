use std::collections::BTreeMap;

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;
use super::test_support::{ConfigLocation, GitFixture};

#[tokio::test]
async fn native_credentials_cannot_corrupt_revisions_or_tree_ids() {
    let mut fixture = GitFixture::new("[core]\n autocrlf = true\n", ConfigLocation::Home);
    for name in ["GIT_AUTHOR_DATE", "GIT_COMMITTER_DATE"] {
        fixture
            .environment
            .insert(name.to_owned(), "1234567890 +0000".to_owned());
    }
    let base = fixture.seed();
    fixture.edit();
    // Compute the ordinary Git result so the collision is deterministic on every platform.
    fixture.git(&["add", "--all"]);
    fixture.git(&[
        "-c",
        "user.name=Zeroshot",
        "-c",
        "user.email=delivery@zeroshot.invalid",
        "commit",
        "-m",
        "fix: one line",
    ]);
    let revision = String::from_utf8(fixture.git(&["rev-parse", "HEAD"]))
        .assert_value()
        .trim()
        .to_owned();
    let tree = String::from_utf8(fixture.git(&["rev-parse", "HEAD^{tree}"]))
        .assert_value()
        .trim()
        .to_owned();
    fixture.git(&["reset", "--mixed", &base]);
    let username = revision[..1].to_owned();
    fixture
        .environment
        .insert("GIT_USERNAME".to_owned(), username.clone());
    fixture
        .environment
        .insert("GIT_PASSWORD".to_owned(), tree.clone());
    let system = fixture.system_git();
    fixture
        .assert_delivered(&system, &base, b"first\nchanged\n")
        .await;
    assert_eq!(
        fixture.git(&["rev-parse", "HEAD"]),
        format!("{revision}\n").as_bytes()
    );
    assert_eq!(
        system
            .revision_tree(&fixture.workspace, "HEAD")
            .await
            .assert_value(),
        tree
    );
    for (reference, expected) in [("HEAD", &revision), ("HEAD^{tree}", &tree)] {
        let output = system
            .execute(&fixture.workspace, &["rev-parse", reference])
            .await
            .assert_value()
            .require_success()
            .assert_value();
        assert_eq!(output.stdout(), format!("{expected}\n"));
        assert_eq!(
            output.diagnostic().stdout,
            command::redact(output.stdout(), &[username.clone(), tree.clone()])
        );
        assert!(!format!("{output:?}\n{output}").contains(expected));
    }
}

#[tokio::test]
async fn native_credential_path_collision_cannot_hide_unfinished_git_operations() {
    let mut fixture = GitFixture::new("[core]\n autocrlf = true\n", ConfigLocation::Home);
    let base = fixture.seed();
    fixture.edit();
    fixture
        .environment
        .insert("GIT_USERNAME".to_owned(), ".git".to_owned());
    let system = fixture.system_git();
    let output = system
        .require_success(&fixture.workspace, &["rev-parse", "--absolute-git-dir"])
        .await
        .assert_value();
    assert!(output.stdout().contains(".git"));
    assert!(!output.diagnostic().stdout.contains(".git"));
    assert!(!format!("{output:?}\n{output}").contains(".git"));
    std::fs::write(
        fixture.workspace.join(".git/CHERRY_PICK_HEAD"),
        format!("{base}\n"),
    )
    .assert_value();
    for error in [
        system
            .workspace_state(&fixture.workspace)
            .await
            .unwrap_err(),
        system
            .prepare_revision(&fixture.workspace, &base, "must not commit")
            .await
            .unwrap_err(),
    ] {
        assert!(matches!(&error, GitError::Command(_)));
        assert!(error.to_string().contains("CHERRY_PICK_HEAD"), "{error}");
        assert!(
            error
                .to_string()
                .contains("no files were staged or committed"),
            "{error}"
        );
    }
    assert_eq!(
        fixture.git(&["rev-parse", "HEAD"]),
        format!("{base}\n").as_bytes()
    );
    assert!(fixture.git(&["diff", "--cached", "--name-only"]).is_empty());
    assert_eq!(
        std::fs::read(fixture.workspace.join("sample.txt")).assert_value(),
        b"first\r\nchanged\r\n"
    );
}

#[tokio::test]
async fn local_git_normalizes_crlf_using_native_global_config_lookup() {
    for setting in ["true", "input"] {
        for location in [
            ConfigLocation::Home,
            ConfigLocation::Xdg,
            ConfigLocation::Explicit,
        ] {
            let fixture = GitFixture::new(&format!("[core]\n autocrlf = {setting}\n"), location);
            let base = fixture.seed();
            assert_eq!(
                fixture.git(&["show", "HEAD:sample.txt"]),
                b"first\nsecond\n"
            );
            fixture.edit();
            fixture
                .assert_delivered(&fixture.system_git(), &base, b"first\nchanged\n")
                .await;
        }
    }
}

#[tokio::test]
async fn local_git_honors_repository_and_attribute_precedence() {
    for (repository_setting, attributes, expected) in [
        ("false", "", b"first\r\nchanged\r\n".as_slice()),
        (
            "false",
            "sample.txt text eol=lf\n",
            b"first\nchanged\n".as_slice(),
        ),
        (
            "true",
            "sample.txt -text\n",
            b"first\r\nchanged\r\n".as_slice(),
        ),
    ] {
        let fixture = GitFixture::new("[core]\n autocrlf = true\n", ConfigLocation::Home);
        fixture.git(&["config", "--local", "core.autocrlf", repository_setting]);
        std::fs::write(fixture.workspace.join(".gitattributes"), attributes).assert_value();
        let base = fixture.seed();
        fixture.edit();
        fixture
            .assert_delivered(&fixture.system_git(), &base, expected)
            .await;
    }
}

#[tokio::test]
async fn local_git_preserves_authored_config_environment_overrides() {
    for overrides in [
        vec![
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", "core.autocrlf"),
            ("GIT_CONFIG_VALUE_0", "false"),
        ],
        vec![("GIT_CONFIG_PARAMETERS", "'core.autocrlf=false'")],
    ] {
        let mut fixture = GitFixture::new("[core]\n autocrlf = true\n", ConfigLocation::Home);
        fixture.environment.extend(
            overrides
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned())),
        );
        let base = fixture.seed();
        fixture.edit();
        fixture
            .assert_delivered(&fixture.system_git(), &base, b"first\r\nchanged\r\n")
            .await;
    }
    for no_system in [None, Some("0"), Some("1")] {
        let mut fixture = GitFixture::new("", ConfigLocation::Home);
        let system_config = fixture.root.path().join("system.gitconfig");
        std::fs::write(&system_config, "[core]\n autocrlf = true\n").assert_value();
        fixture.environment.remove("GIT_CONFIG_NOSYSTEM");
        if let Some(value) = no_system {
            fixture
                .environment
                .insert("GIT_CONFIG_NOSYSTEM".to_owned(), value.to_owned());
        }
        fixture.environment.insert(
            "GIT_CONFIG_SYSTEM".to_owned(),
            system_config.to_str().assert_value().to_owned(),
        );
        let base = fixture.seed();
        fixture.edit();
        let expected = if no_system == Some("1") {
            b"first\r\nchanged\r\n".as_slice()
        } else {
            b"first\nchanged\n".as_slice()
        };
        fixture
            .assert_delivered(&fixture.system_git(), &base, expected)
            .await;
    }
}

#[cfg(windows)]
#[tokio::test]
async fn local_git_preserves_case_insensitive_windows_config_overrides() {
    let mut explicit = GitFixture::new("[core]\n autocrlf = true\n", ConfigLocation::Explicit);
    let global = explicit
        .environment
        .remove("GIT_CONFIG_GLOBAL")
        .assert_value();
    explicit
        .environment
        .insert("git_config_global".to_owned(), global);
    let base = explicit.seed();
    explicit.edit();
    explicit
        .assert_delivered(&explicit.system_git(), &base, b"first\nchanged\n")
        .await;

    let mut no_system = GitFixture::new("", ConfigLocation::Home);
    no_system.environment.remove("GIT_CONFIG_NOSYSTEM");
    no_system
        .environment
        .insert("git_config_nosystem".to_owned(), "1".to_owned());
    let system_config = no_system.root.path().join("system.gitconfig");
    std::fs::write(&system_config, "[core]\n autocrlf = true\n").assert_value();
    no_system.environment.insert(
        "GIT_CONFIG_SYSTEM".to_owned(),
        system_config.to_str().assert_value().to_owned(),
    );
    let base = no_system.seed();
    no_system.edit();
    no_system
        .assert_delivered(&no_system.system_git(), &base, b"first\r\nchanged\r\n")
        .await;
}

#[tokio::test]
async fn local_git_keeps_fixed_identity_disabled_hooks_and_selected_repository() {
    let mut fixture = GitFixture::new("[core]\n autocrlf = true\n", ConfigLocation::Home);
    let base = fixture.seed();
    let hook = fixture.workspace.join(".git/hooks/pre-commit");
    #[cfg(unix)]
    openengine_cluster_testkit::fixture::write_executable(&hook, "#!/bin/sh\nexit 93\n", 0o700)
        .assert_value();
    #[cfg(not(unix))]
    std::fs::write(&hook, "#!/bin/sh\nexit 93\n").assert_value();
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "GIT_NAMESPACE",
        "ZEROSHOT_API_KEY",
    ] {
        fixture
            .environment
            .insert(variable.to_owned(), "must-not-reach-git".to_owned());
    }
    for variable in [
        "GIT_AUTHOR_NAME",
        "GIT_AUTHOR_EMAIL",
        "GIT_COMMITTER_NAME",
        "GIT_COMMITTER_EMAIL",
    ] {
        fixture
            .environment
            .insert(variable.to_owned(), "caller-identity".to_owned());
    }
    let system = fixture.system_git();
    let command = system.command(&fixture.workspace, &["status"]);
    let environment: BTreeMap<_, _> = command.as_std().get_envs().collect();
    assert!(
        !environment
            .values()
            .any(|value| value.is_some_and(|value| value == "must-not-reach-git"))
    );
    // Keep ordinary fixture inspection isolated from the deliberately hostile snapshot.
    fixture
        .environment
        .retain(|key, _| !key.starts_with("GIT_") || key == "GIT_CONFIG_NOSYSTEM");
    fixture.edit();
    fixture
        .assert_delivered(&system, &base, b"first\nchanged\n")
        .await;
    assert_eq!(
        fixture.git(&["show", "-s", "--format=%an <%ae>|%cn <%ce>"]),
        b"Zeroshot <delivery@zeroshot.invalid>|Zeroshot <delivery@zeroshot.invalid>\n"
    );
}

#[tokio::test]
async fn hosted_git_keeps_global_and_system_config_isolated() {
    let fixture = GitFixture::new("[core]\n autocrlf = true\n", ConfigLocation::Home);
    let base = fixture.seed();
    fixture.edit();
    let system = SystemGit::new(fixture.program.clone());
    let command = system.command(&fixture.workspace, &["status"]);
    let environment: BTreeMap<_, _> = command.as_std().get_envs().collect();
    for variable in [
        "HOME",
        "USERPROFILE",
        "XDG_CONFIG_HOME",
        "GIT_CONFIG_SYSTEM",
    ] {
        assert!(!environment.contains_key(std::ffi::OsStr::new(variable)));
    }
    assert_eq!(
        environment[std::ffi::OsStr::new("GIT_CONFIG_NOSYSTEM")],
        Some(std::ffi::OsStr::new("1"))
    );
    assert_eq!(
        environment[std::ffi::OsStr::new("GIT_CONFIG_GLOBAL")],
        Some(std::ffi::OsStr::new("/dev/null"))
    );
    // Even with a host home/system path present, hosted isolation still wins.
    let system_config = fixture.root.path().join("system.gitconfig");
    std::fs::write(&system_config, "[core]\n autocrlf = true\n").assert_value();
    let mut hosted = system.command(&fixture.workspace, &["add", "--all"]);
    hosted.env("GIT_CONFIG_SYSTEM", system_config);
    hosted.envs(
        fixture
            .environment
            .iter()
            .filter(|(key, _)| key.as_str() != "GIT_CONFIG_NOSYSTEM"),
    );
    command::capture(&mut hosted, Duration::from_secs(30))
        .await
        .assert_value()
        .require_success()
        .assert_value();
    let revision = system
        .prepare_revision(&fixture.workspace, &base, "hosted bytes")
        .await
        .assert_value();
    assert_eq!(
        fixture.git(&["show", &format!("{revision}:sample.txt")]),
        b"first\r\nchanged\r\n"
    );
}
