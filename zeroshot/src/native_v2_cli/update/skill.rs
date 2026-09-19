use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{NativeV2CliError, update_error};

const SKILL_NAME: &str = "zeroshot";
const SKILL_FILENAME: &str = "SKILL.md";
const MANAGED_PREFIX: &[u8] = b"<!-- managed by @the-open-engine-company/zeroshot; sha256=";
const MANAGED_SUFFIX: &[u8] = b" -->\n";
const MAX_SKILL_BYTES: u64 = 1024 * 1024;

pub(super) struct PreparedSkill {
    canonical: Vec<u8>,
    managed: Vec<u8>,
}

pub(super) fn prepare(canonical: Vec<u8>) -> Result<PreparedSkill, NativeV2CliError> {
    if canonical.len() as u64 > MAX_SKILL_BYTES {
        return Err(update_error("release skill exceeds the size limit"));
    }
    std::str::from_utf8(&canonical).map_err(|_| update_error("release skill is not UTF-8"))?;
    let managed = managed_document(&canonical)?;
    Ok(PreparedSkill { canonical, managed })
}

impl PreparedSkill {
    pub(super) fn install(&self) -> Result<bool, NativeV2CliError> {
        reject_sudo_install()?;
        let home = crate::execution::platform::user_home()
            .ok_or_else(|| update_error("user home is unavailable for the skill update"))?;
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|value| !value.is_empty());
        self.install_for(&home, claude.as_deref())
    }

    fn install_for(
        &self,
        home: &Path,
        claude_config: Option<&OsStr>,
    ) -> Result<bool, NativeV2CliError> {
        if !home.is_absolute() {
            return Err(update_error(
                "user home must be absolute for the skill update",
            ));
        }
        let mut changed = false;
        let mut failures = Vec::new();
        let mut progress = InstallProgress {
            changed: &mut changed,
            failures: &mut failures,
        };
        let agents = home.join(".agents").join("skills").join(SKILL_NAME);
        install_location("Codex/GitHub Copilot", &agents, self, &mut progress);

        let claude_root = claude_config
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"));
        if claude_root.is_absolute() {
            let claude = claude_root.join("skills").join(SKILL_NAME);
            install_location("Claude Code", &claude, self, &mut progress);
        } else {
            progress
                .failures
                .push("Claude Code: CLAUDE_CONFIG_DIR must be absolute".to_owned());
        }

        if failures.is_empty() {
            Ok(changed)
        } else {
            Err(update_error(format!(
                "skill update incomplete: {}; resolve these paths and re-run `zeroshot update`",
                failures.join("; ")
            )))
        }
    }
}

struct InstallProgress<'a> {
    changed: &'a mut bool,
    failures: &'a mut Vec<String>,
}

fn install_location(
    label: &str,
    directory: &Path,
    skill: &PreparedSkill,
    progress: &mut InstallProgress<'_>,
) {
    let filename = directory.join(SKILL_FILENAME);
    match install_at(directory, &filename, &skill.canonical, &skill.managed) {
        Ok(installed) => *progress.changed |= installed,
        Err(message) => progress
            .failures
            .push(format!("{label} ({}): {message}", filename.display())),
    }
}

fn install_at(
    directory: &Path,
    filename: &Path,
    canonical: &[u8],
    managed: &[u8],
) -> Result<bool, String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("could not create directory: {error}"))?;
    let directory_metadata = fs::symlink_metadata(directory)
        .map_err(|error| format!("could not inspect skill directory: {error}"))?;
    if !directory_metadata.is_dir() || directory_metadata.file_type().is_symlink() {
        return Err("skill directory is not a regular directory".to_owned());
    }

    match inspect_existing(filename, canonical, managed)? {
        ExistingSkill::Unchanged => Ok(false),
        ExistingSkill::Conflict(message) => Err(message),
        ExistingSkill::Install => {
            atomic_write(directory, filename, managed)?;
            Ok(true)
        }
    }
}

enum ExistingSkill {
    Install,
    Unchanged,
    Conflict(String),
}

fn inspect_existing(
    filename: &Path,
    canonical: &[u8],
    managed: &[u8],
) -> Result<ExistingSkill, String> {
    let Some(existing) = read_existing(filename)? else {
        return Ok(ExistingSkill::Install);
    };
    if existing == managed {
        return Ok(ExistingSkill::Unchanged);
    }
    if existing == canonical || managed_source(&existing).is_some() {
        return Ok(ExistingSkill::Install);
    }
    Ok(ExistingSkill::Conflict(
        "existing skill is not an unmodified Zeroshot-managed copy".to_owned(),
    ))
}

fn read_existing(filename: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = fs::symlink_metadata(filename);
    if metadata
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(None);
    }
    let metadata = metadata.map_err(|error| format!("could not inspect skill: {error}"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("SKILL.md is not a regular file".to_owned());
    }
    if metadata.len() > MAX_SKILL_BYTES {
        return Err("existing skill exceeds the size limit".to_owned());
    }
    fs::read(filename)
        .map(Some)
        .map_err(|error| format!("could not read skill: {error}"))
}

fn atomic_write(directory: &Path, filename: &Path, contents: &[u8]) -> Result<(), String> {
    let mut temporary = tempfile::Builder::new()
        .prefix(".SKILL.md.")
        .suffix(".tmp")
        .tempfile_in(directory)
        .map_err(|error| format!("could not stage skill: {error}"))?;
    temporary
        .write_all(contents)
        .map_err(|error| format!("could not stage skill: {error}"))?;
    make_public(temporary.path())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("could not stage skill: {error}"))?;
    temporary
        .into_temp_path()
        .persist(filename)
        .map_err(|error| format!("could not replace skill: {}", error.error))
}

#[cfg(unix)]
fn make_public(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o644))
        .map_err(|error| format!("could not set skill permissions: {error}"))
}

#[cfg(windows)]
fn make_public(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn managed_document(canonical: &[u8]) -> Result<Vec<u8>, NativeV2CliError> {
    let offset = frontmatter_end(canonical)
        .ok_or_else(|| update_error("release skill has invalid YAML frontmatter"))?;
    if canonical[offset..].starts_with(MANAGED_PREFIX) {
        return Err(update_error(
            "release skill already contains a managed marker",
        ));
    }
    let marker = format!(
        "<!-- managed by @the-open-engine-company/zeroshot; sha256={:x} -->\n",
        Sha256::digest(canonical)
    );
    let mut document = Vec::with_capacity(canonical.len() + marker.len());
    document.extend_from_slice(&canonical[..offset]);
    document.extend_from_slice(marker.as_bytes());
    document.extend_from_slice(&canonical[offset..]);
    Ok(document)
}

fn managed_source(document: &[u8]) -> Option<Vec<u8>> {
    let offset = frontmatter_end(document)?;
    let remainder = &document[offset..];
    let digest_start = MANAGED_PREFIX.len();
    let digest_end = digest_start + 64;
    if !remainder.starts_with(MANAGED_PREFIX)
        || remainder.len() < digest_end + MANAGED_SUFFIX.len()
        || !remainder[digest_start..digest_end]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        || &remainder[digest_end..digest_end + MANAGED_SUFFIX.len()] != MANAGED_SUFFIX
    {
        return None;
    }
    let mut source =
        Vec::with_capacity(document.len() - MANAGED_PREFIX.len() - 64 - MANAGED_SUFFIX.len());
    source.extend_from_slice(&document[..offset]);
    source.extend_from_slice(&remainder[digest_end + MANAGED_SUFFIX.len()..]);
    let actual = format!("{:x}", Sha256::digest(&source));
    (actual.as_bytes() == &remainder[digest_start..digest_end]).then_some(source)
}

fn frontmatter_end(document: &[u8]) -> Option<usize> {
    document
        .starts_with(b"---\n")
        .then(|| {
            document[4..]
                .windows(5)
                .position(|window| window == b"\n---\n")
                .map(|position| position + 9)
        })
        .flatten()
}

#[cfg(unix)]
fn reject_sudo_install() -> Result<(), NativeV2CliError> {
    let sudo_user = std::env::var_os("SUDO_USER");
    if unsafe { libc::geteuid() } == 0
        && sudo_user
            .as_deref()
            .is_some_and(|user| user != OsStr::new("root"))
    {
        return Err(update_error(
            "refusing to update user skills through sudo; use a user-owned installation",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn reject_sudo_install() -> Result<(), NativeV2CliError> {
    Ok(())
}

#[cfg(test)]
mod tests {
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
    fn managed_document_round_trips_only_unchanged_source() {
        let prepared = prepare(FIRST.as_bytes().to_vec()).unwrap();
        assert_eq!(managed_source(&prepared.managed).unwrap(), FIRST.as_bytes());
        let mut modified = prepared.managed.clone();
        modified.extend_from_slice(b"user edit\n");
        assert!(managed_source(&modified).is_none());
        assert!(prepare(b"not frontmatter\n".to_vec()).is_err());
    }

    #[test]
    fn installs_reuses_and_updates_both_managed_copies() {
        let (_root, home, first) = fixture(FIRST);
        assert!(first.install_for(&home, None).unwrap());
        let (agents, claude) = destinations(&home);
        assert_eq!(fs::read(&agents).unwrap(), fs::read(&claude).unwrap());
        assert!(!first.install_for(&home, None).unwrap());

        let second = prepare(SECOND.as_bytes().to_vec()).unwrap();
        assert!(second.install_for(&home, None).unwrap());
        assert_source(&agents, SECOND.as_bytes());
        assert_source(&claude, SECOND.as_bytes());
    }

    #[test]
    fn preserves_modified_skills_and_updates_other_managed_copies() {
        let (_root, home, first) = fixture(FIRST);
        first.install_for(&home, None).unwrap();
        let (agents, claude) = destinations(&home);
        fs::write(&agents, b"user-owned\n").unwrap();

        let second = prepare(SECOND.as_bytes().to_vec()).unwrap();
        let error = second.install_for(&home, None).unwrap_err().to_string();
        assert!(error.contains("Codex/GitHub Copilot"));
        assert_reported_path(&error, &agents);
        assert!(error.contains("existing skill is not an unmodified Zeroshot-managed copy"));
        assert_eq!(fs::read(agents).unwrap(), b"user-owned\n");
        assert_source(&claude, SECOND.as_bytes());
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
    fn does_not_follow_a_conflicting_skill_directory_symlink() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let outside = root.path().join("outside");
        fs::create_dir_all(home.join(".agents/skills")).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, home.join(".agents/skills/zeroshot")).unwrap();
        let prepared = prepare(FIRST.as_bytes().to_vec()).unwrap();
        let error = prepared.install_for(&home, None).unwrap_err().to_string();
        assert!(error.contains("skill directory is not a regular directory"));
        assert!(!outside.join("SKILL.md").exists());
    }
}
