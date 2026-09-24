use super::*;

const FIRST: &str = "---\nname: zeroshot\ndescription: Test skill\n---\n\nVersion one.\n";
const SECOND: &str = "---\nname: zeroshot\ndescription: Test skill\n---\n\nVersion two.\n";

fn fixture(source: &str) -> (tempfile::TempDir, PathBuf, PreparedSkill) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    fs::create_dir(&home).unwrap();
    let prepared = prepare(source.as_bytes().to_vec()).unwrap();
    (root, home, prepared)
}

fn destinations(home: &Path) -> (PathBuf, PathBuf) {
    (
        home.join(".agents/skills/zeroshot/SKILL.md"),
        home.join(".claude/skills/zeroshot/SKILL.md"),
    )
}

fn assert_source(path: &Path, expected: &[u8]) {
    assert_eq!(managed_source(&fs::read(path).unwrap()).unwrap(), expected);
}

fn assert_reported_path(error: &str, path: &Path) {
    let expected = path.display().to_string();
    #[cfg(windows)]
    {
        let normalize = |value: &str| {
            value
                .replace("\\\\?\\", "")
                .replace('\\', "/")
                .to_ascii_lowercase()
        };
        assert!(normalize(error).contains(&normalize(&expected)), "{error}");
    }
    #[cfg(not(windows))]
    assert!(error.contains(&expected), "{error}");
}

#[test]
fn managed_document_round_trips_only_unchanged_valid_source() {
    let prepared = prepare(FIRST.as_bytes().to_vec()).unwrap();
    assert_eq!(managed_source(&prepared.managed).unwrap(), FIRST.as_bytes());

    let mut modified = prepared.managed.clone();
    modified.extend_from_slice(b"user edit\n");
    assert!(managed_source(&modified).is_none());
    assert!(prepare(modified).is_err());
    assert!(prepare(b"not frontmatter\n".to_vec()).is_err());
    assert!(prepare(vec![0xff]).is_err());
    assert!(prepare(vec![b'x'; MAX_SKILL_BYTES as usize + 1]).is_err());

    for malformed in [
        concat!(
            "---\nname: zeroshot\n---\n",
            "<!-- managed by @the-open-engine-company/zeroshot; ",
            "sha256=short -->\nbody\n"
        )
        .as_bytes(),
        concat!(
            "---\nname: zeroshot\n---\n",
            "<!-- managed by @the-open-engine-company/zeroshot; sha256=",
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            " -->\nbody\n"
        )
        .as_bytes(),
        concat!(
            "---\nname: zeroshot\n---\n",
            "<!-- managed by @the-open-engine-company/zeroshot; sha256=",
            "0000000000000000000000000000000000000000000000000000000000000000",
            " -->body\n"
        )
        .as_bytes(),
    ] {
        assert!(managed_source(malformed).is_none());
    }
}

#[test]
fn wave5_cli_contract_skill_rejects_unbounded_files_and_relative_homes() {
    let (root, home, prepared) = fixture(FIRST);
    assert!(prepared.install_for(Path::new("relative"), None).is_err());

    let agents = destinations(&home).0;
    fs::create_dir_all(agents.parent().unwrap()).unwrap();
    let oversized = fs::File::create(&agents).unwrap();
    oversized.set_len(MAX_SKILL_BYTES + 1).unwrap();
    let error = prepared.install_for(&home, None).unwrap_err().to_string();
    assert!(error.contains("existing skill exceeds the size limit"));
    assert_source(&destinations(&home).1, FIRST.as_bytes());
    drop(root);
}

#[test]
fn installs_reuses_adopts_and_upgrades_both_managed_copies() {
    let (_root, home, first) = fixture(FIRST);
    assert!(first.install_for(&home, None).unwrap());
    let (agents, claude) = destinations(&home);
    assert_eq!(fs::read(&agents).unwrap(), fs::read(&claude).unwrap());
    assert!(!first.install_for(&home, None).unwrap());

    let second = prepare(SECOND.as_bytes().to_vec()).unwrap();
    assert!(second.install_for(&home, None).unwrap());
    assert_source(&agents, SECOND.as_bytes());
    assert_source(&claude, SECOND.as_bytes());

    fs::write(&agents, SECOND).unwrap();
    assert!(second.install_for(&home, None).unwrap());
    assert_source(&agents, SECOND.as_bytes());
}

#[test]
fn refuses_unmanaged_or_tampered_skills_and_updates_the_other_managed_copy() {
    let (_root, home, first) = fixture(FIRST);
    first.install_for(&home, None).unwrap();
    let (agents, claude) = destinations(&home);
    fs::write(&agents, b"user-owned\n").unwrap();

    let second = prepare(SECOND.as_bytes().to_vec()).unwrap();
    let error = second.install_for(&home, None).unwrap_err().to_string();
    assert!(error.contains("Codex/GitHub Copilot"));
    assert_reported_path(&error, &agents);
    assert!(error.contains("existing skill is not an unmodified Zeroshot-managed copy"));
    assert_eq!(fs::read(&agents).unwrap(), b"user-owned\n");
    assert_source(&claude, SECOND.as_bytes());

    fs::write(&agents, &first.managed).unwrap();
    fs::write(&claude, &first.managed).unwrap();
    let mut tampered = first.managed.clone();
    tampered.extend_from_slice(b"tampered\n");
    fs::write(&agents, &tampered).unwrap();
    let error = second.install_for(&home, None).unwrap_err().to_string();
    assert!(error.contains("existing skill is not an unmodified Zeroshot-managed copy"));
    assert_eq!(fs::read(&agents).unwrap(), tampered);
}

#[test]
fn custom_claude_config_is_preserved_without_touching_the_default_directory() {
    let (root, home, prepared) = fixture(FIRST);
    let custom = root.path().join("custom-claude");
    fs::create_dir(&custom).unwrap();
    fs::write(custom.join("settings.json"), b"preserve me").unwrap();

    assert!(
        prepared
            .install_for(&home, Some(custom.as_os_str()))
            .unwrap()
    );
    assert!(destinations(&home).0.is_file());
    assert_source(&custom.join("skills/zeroshot/SKILL.md"), FIRST.as_bytes());
    assert_eq!(
        fs::read(custom.join("settings.json")).unwrap(),
        b"preserve me"
    );
    assert!(!home.join(".claude").exists());
}

#[test]
fn rejects_relative_paths_without_hiding_a_successful_destination() {
    let (_root, home, prepared) = fixture(FIRST);
    let error = prepared
        .install_for(&home, Some(OsStr::new("relative")))
        .unwrap_err()
        .to_string();
    assert!(error.contains("CLAUDE_CONFIG_DIR must be absolute"));
    assert!(destinations(&home).0.is_file());
    assert!(!destinations(&home).1.exists());
}

#[cfg(unix)]
#[test]
fn does_not_follow_skill_directory_or_file_symlinks() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("directory-home");
    let outside = root.path().join("outside-directory");
    fs::create_dir_all(home.join(".agents/skills")).unwrap();
    fs::create_dir(&outside).unwrap();
    symlink(&outside, home.join(".agents/skills/zeroshot")).unwrap();
    let prepared = prepare(FIRST.as_bytes().to_vec()).unwrap();
    let error = prepared.install_for(&home, None).unwrap_err().to_string();
    assert!(error.contains("skill directory is not a regular directory"));
    assert!(!outside.join("SKILL.md").exists());

    let home = root.path().join("file-home");
    let directory = home.join(".agents/skills/zeroshot");
    let outside = root.path().join("outside-skill.md");
    fs::create_dir_all(&directory).unwrap();
    fs::write(&outside, b"outside remains unchanged").unwrap();
    symlink(&outside, directory.join("SKILL.md")).unwrap();
    let error = prepared.install_for(&home, None).unwrap_err().to_string();
    assert!(error.contains("SKILL.md is not a regular file"));
    assert_eq!(fs::read(&outside).unwrap(), b"outside remains unchanged");
}

#[cfg(unix)]
#[test]
fn wave9_cli_contract_skill_install_guard_matches_effective_sudo_identity() {
    for (effective_root, sudo_user, refused) in [
        (false, Some(OsStr::new("alice")), false),
        (true, None, false),
        (true, Some(OsStr::new("root")), false),
        (true, Some(OsStr::new("alice")), true),
    ] {
        assert_eq!(
            reject_sudo_install_for(effective_root, sudo_user).is_err(),
            refused
        );
    }

    let sudo_user = std::env::var_os("SUDO_USER");
    let should_refuse = unsafe { libc::geteuid() } == 0
        && sudo_user
            .as_deref()
            .is_some_and(|user| user != OsStr::new("root"));
    assert_eq!(reject_sudo_install().is_err(), should_refuse);
}
