use std::collections::BTreeSet;
use std::fmt;
use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[path = "update/skill.rs"]
mod skill;

use super::{CliOutcome, NativeV2CliError};
use crate::native_v2_runner::bounded_log_text;

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/the-open-engine/zeroshot/releases/latest";
const RELEASE_BASE_URL: &str = "https://github.com/the-open-engine/zeroshot/releases/download";
const MINIMUM_RELEASE_MAJOR: u64 = 8;
const MAX_RELEASE_BYTES: usize = 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_SKILL_BYTES: usize = 1024 * 1024;
const MAX_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;
const SKILL_ASSET: &str = "zeroshot-skill.md";
const SKILL_ASSETS: [(&str, &str); 4] = [
    ("zeroshot", SKILL_ASSET),
    ("zeroshot-memory-init", "zeroshot-memory-init-skill.md"),
    ("zeroshot-graph-design", "zeroshot-graph-design-skill.md"),
    ("zeroshot-plan", "zeroshot-plan-skill.md"),
];
const RESTIC_VERSION: &str = "0.19.1";
/// Name prefixes of the executables an update stages next to the installed one.
const STAGED_ZEROSHOT_PREFIX: &str = ".zeroshot-update-";
const STAGED_RESTIC_PREFIX: &str = ".restic-update-";
/// Serializes the updates of one installation. No cleanup pattern matches its name.
const UPDATE_LOCK: &str = ".zeroshot-update.lock";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest wait for a response, then for each next part of its body. A download that keeps
/// receiving data is never cut off, however slow, while one that stops fails as a timeout.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_FAILURE_BODY_BYTES: usize = 64 * 1024;
const MAX_FAILURE_MESSAGE_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ReleaseVersion([u64; 3]);

impl ReleaseVersion {
    fn parse(value: &str) -> Option<Self> {
        let parts = value
            .split('.')
            .map(str::parse)
            .collect::<Result<Vec<u64>, _>>()
            .ok()?;
        (parts.len() == 3).then(|| Self([parts[0], parts[1], parts[2]]))
    }
}

impl fmt::Display for ReleaseVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.0[0], self.0[1], self.0[2])
    }
}

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
}

/// The explanation GitHub returns with a failed request, such as an exhausted rate limit.
#[derive(Debug, Deserialize)]
struct GitHubFailure {
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateResult {
    current_version: String,
    latest_version: String,
    updated: bool,
    skill_updated: bool,
}

struct ReleaseUpdate {
    executables: Option<ReleaseExecutables>,
    skills: Vec<skill::PreparedSkill>,
}

struct ReleaseExecutables {
    zeroshot: Vec<u8>,
    restic: Vec<u8>,
    restic_name: &'static str,
}

struct InstalledRelease<'a> {
    executable: &'a Path,
    version: ReleaseVersion,
}

pub(super) async fn execute(output: &mut impl Write) -> Result<CliOutcome, NativeV2CliError> {
    let current = installed_version()?;
    let client = http_client()?;
    let latest = latest_version(&client).await?;
    let executable = std::env::current_exe()
        .map_err(|error| update_error(format!("could not locate this executable: {error}")))?;
    let installation = InstalledRelease {
        executable: &executable,
        version: current,
    };
    let base_url = format!("{RELEASE_BASE_URL}/v{latest}");
    let (updated, skill_updated) = apply_release(&client, installation, latest, &base_url).await?;
    write_result(
        output,
        UpdateResult {
            current_version: current.to_string(),
            latest_version: latest.to_string(),
            updated,
            skill_updated,
        },
    )?;
    Ok(CliOutcome::Completed)
}

async fn apply_release(
    client: &reqwest::Client,
    current: InstalledRelease<'_>,
    latest: ReleaseVersion,
    base_url: &str,
) -> Result<(bool, bool), NativeV2CliError> {
    if latest < current.version {
        return Ok((false, false));
    }
    check_installation(current.executable, current.version, latest)?;
    let release = download_release_at(client, current.version, latest, base_url).await?;
    let updated = match &release.executables {
        Some(executables) => install(current.executable, executables, latest)?,
        None => false,
    };
    let skill_updated = install_skills(&release.skills)?;
    Ok((updated, skill_updated))
}

fn installed_version() -> Result<ReleaseVersion, NativeV2CliError> {
    let version = release_version(env!("CARGO_PKG_VERSION"), "installed version")?;
    if version.0[0] >= MINIMUM_RELEASE_MAJOR {
        Ok(version)
    } else {
        Err(update_error(
            "self-update is available only in canonical v8 or newer release builds",
        ))
    }
}

fn http_client() -> Result<reqwest::Client, NativeV2CliError> {
    reqwest::Client::builder()
        .redirect(Policy::limited(5))
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(IDLE_TIMEOUT)
        .user_agent("zeroshot self-update")
        .build()
        .map_err(|error| {
            update_error(format!(
                "could not initialize HTTPS: {}",
                http_failure(error)
            ))
        })
}

async fn latest_version(client: &reqwest::Client) -> Result<ReleaseVersion, NativeV2CliError> {
    latest_version_at(client, LATEST_RELEASE_URL).await
}

async fn latest_version_at(
    client: &reqwest::Client,
    url: &str,
) -> Result<ReleaseVersion, NativeV2CliError> {
    let response = fetch(client, url, "latest release").await?;
    let status = response.status();
    let metadata = download_response(response, MAX_RELEASE_BYTES, "latest release")
        .await
        .map_err(|error| {
            if matches!(
                status,
                reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::TOO_MANY_REQUESTS
            ) {
                with_npm_hint(error, None)
            } else {
                error
            }
        })?;
    let release: LatestRelease = serde_json::from_slice(&metadata)
        .map_err(|error| update_error(format!("latest release metadata is invalid: {error}")))?;
    let tag_version = release
        .tag_name
        .strip_prefix('v')
        .ok_or_else(|| update_error("latest release tag is not canonical"))?;
    let latest = release_version(tag_version, "latest release")?;
    if release.tag_name != format!("v{latest}") || latest.0[0] < MINIMUM_RELEASE_MAJOR {
        return Err(update_error("latest release tag is not canonical"));
    }
    Ok(latest)
}

async fn download_release_at(
    client: &reqwest::Client,
    current: ReleaseVersion,
    latest: ReleaseVersion,
    base_url: &str,
) -> Result<ReleaseUpdate, NativeV2CliError> {
    let manifest = download(
        client,
        &format!("{base_url}/SHA256SUMS"),
        MAX_MANIFEST_BYTES,
        "release checksum manifest",
    )
    .await?;
    let release = VerifiedRelease {
        client,
        base_url,
        manifest: &manifest,
    };
    let skills = download_skills(&release).await?;
    let executables = if latest > current {
        let (target, executable, restic) = release_target()?;
        let filename = format!("zeroshot-v{latest}-{target}.tar.gz");
        let archive = release.download(&filename, MAX_ARCHIVE_BYTES).await?;
        let mut extracted = extract_executables(&archive, &[executable, restic], latest)?;
        Some(ReleaseExecutables {
            zeroshot: extracted
                .remove(executable)
                .ok_or_else(|| update_error("release archive omitted Zeroshot"))?,
            restic: extracted
                .remove(restic)
                .ok_or_else(|| update_error("release archive omitted Restic"))?,
            restic_name: restic,
        })
    } else {
        None
    };
    Ok(ReleaseUpdate {
        executables,
        skills,
    })
}

async fn download_skills(
    release: &VerifiedRelease<'_>,
) -> Result<Vec<skill::PreparedSkill>, NativeV2CliError> {
    if std::env::var_os("ZEROSHOT_SKIP_SKILLS").as_deref() == Some(std::ffi::OsStr::new("1")) {
        return Ok(Vec::new());
    }
    let mut skills = Vec::new();
    for (name, filename) in SKILL_ASSETS {
        skills.push(skill::prepare_named(
            name,
            release.download(filename, MAX_SKILL_BYTES).await?,
        )?);
    }
    Ok(skills)
}

fn install_skills(skills: &[skill::PreparedSkill]) -> Result<bool, NativeV2CliError> {
    let mut changed = false;
    let mut failures = Vec::new();
    for skill in skills {
        match skill.install() {
            Ok(updated) => changed |= updated,
            Err(error) => failures.push(error.to_string()),
        }
    }
    if failures.is_empty() {
        Ok(changed)
    } else {
        Err(update_error(failures.join("; ")))
    }
}

struct VerifiedRelease<'a> {
    client: &'a reqwest::Client,
    base_url: &'a str,
    manifest: &'a [u8],
}

impl VerifiedRelease<'_> {
    async fn download(&self, filename: &str, maximum: usize) -> Result<Vec<u8>, NativeV2CliError> {
        let expected_checksum = checksum_for(self.manifest, filename)?;
        let contents = download(
            self.client,
            &format!("{}/{filename}", self.base_url),
            maximum,
            filename,
        )
        .await?;
        verify_checksum(filename, &contents, &expected_checksum)?;
        Ok(contents)
    }
}

fn release_version(value: &str, kind: &str) -> Result<ReleaseVersion, NativeV2CliError> {
    let version = ReleaseVersion::parse(value)
        .filter(|version| version.to_string() == value)
        .ok_or_else(|| update_error(format!("{kind} {value:?} is not canonical")))?;
    Ok(version)
}

fn release_target() -> Result<(&'static str, &'static str, &'static str), NativeV2CliError> {
    release_target_for(std::env::consts::OS, std::env::consts::ARCH)
}

fn release_target_for(
    os: &str,
    arch: &str,
) -> Result<(&'static str, &'static str, &'static str), NativeV2CliError> {
    match (os, arch) {
        ("linux", "x86_64") => Ok(("x86_64-unknown-linux-musl", "zeroshot", "restic")),
        ("linux", "aarch64") => Ok(("aarch64-unknown-linux-musl", "zeroshot", "restic")),
        ("macos", "x86_64") => Ok(("x86_64-apple-darwin", "zeroshot", "restic")),
        ("macos", "aarch64") => Ok(("aarch64-apple-darwin", "zeroshot", "restic")),
        ("windows", "x86_64") => Ok(("x86_64-pc-windows-msvc", "zeroshot.exe", "restic.exe")),
        _ => Err(update_error(format!(
            "no prebuilt Zeroshot release exists for {os}/{arch}"
        ))),
    }
}

async fn download(
    client: &reqwest::Client,
    url: &str,
    maximum: usize,
    label: &str,
) -> Result<Vec<u8>, NativeV2CliError> {
    download_response(fetch(client, url, label).await?, maximum, label).await
}

async fn fetch(
    client: &reqwest::Client,
    url: &str,
    label: &str,
) -> Result<reqwest::Response, NativeV2CliError> {
    client
        .get(url)
        .send()
        .await
        .map_err(|error| update_error(format!("could not fetch {label}: {}", http_failure(error))))
}

async fn download_response(
    response: reqwest::Response,
    maximum: usize,
    label: &str,
) -> Result<Vec<u8>, NativeV2CliError> {
    let status = response.status();
    if !status.is_success() {
        let explanation = github_message(response)
            .await
            .map(|message| format!(": {message}"))
            .unwrap_or_default();
        return Err(update_error(format!(
            "could not fetch {label}: HTTP {status}{explanation}"
        )));
    }
    read_bounded(response, maximum)
        .await
        .map_err(|error| update_error(format!("could not read {label}: {}", http_failure(error))))?
        .ok_or_else(|| update_error(format!("{label} exceeds {maximum} bytes")))
}

/// Reads the whole body, or returns `None` as soon as it would exceed `maximum` bytes.
async fn read_bounded(
    response: reqwest::Response,
    maximum: usize,
) -> Result<Option<Vec<u8>>, reqwest::Error> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Ok(None);
    }
    let mut contents = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if contents.len().saturating_add(chunk.len()) > maximum {
            return Ok(None);
        }
        contents.extend_from_slice(&chunk);
    }
    Ok(Some(contents))
}

/// GitHub's explanation of a failed request as bounded, control-safe single-line text.
async fn github_message(response: reqwest::Response) -> Option<String> {
    let body = read_bounded(response, MAX_FAILURE_BODY_BYTES)
        .await
        .ok()
        .flatten()?;
    let failure: GitHubFailure = serde_json::from_slice(&body).ok()?;
    let message =
        bounded_log_text(failure.message, MAX_FAILURE_MESSAGE_BYTES).replace(['\n', '\t'], " ");
    let message = message.trim();
    (!message.is_empty()).then(|| message.to_owned())
}

/// Describes an HTTP failure by its complete cause chain, because reqwest's own text omits the
/// proxy, TLS, connection or timeout cause. URLs stay out: release assets redirect to signed URLs.
fn http_failure(error: reqwest::Error) -> String {
    let error = error.without_url();
    let causes = std::iter::successors(
        Some(&error as &(dyn std::error::Error + 'static)),
        |error| error.source(),
    )
    .map(ToString::to_string)
    .collect::<Vec<_>>()
    .join(": ");
    if !error.is_timeout() {
        return causes;
    }
    let (limit, awaited) = if error.is_connect() {
        (CONNECT_TIMEOUT, "to connect")
    } else {
        (IDLE_TIMEOUT, "for the server")
    };
    format!(
        "timed out after waiting {} seconds {awaited}: {causes}",
        limit.as_secs()
    )
}

fn checksum_for(manifest: &[u8], filename: &str) -> Result<String, NativeV2CliError> {
    let manifest = std::str::from_utf8(manifest)
        .map_err(|_| update_error("release checksum manifest is not UTF-8"))?;
    let mut selected = None;
    let mut names = BTreeSet::new();
    for line in manifest.lines().filter(|line| !line.is_empty()) {
        let (checksum, name) = line
            .split_once("  ")
            .filter(|(checksum, name)| valid_checksum_entry(checksum, name))
            .ok_or_else(|| update_error("release checksum manifest has an invalid line"))?;
        if !names.insert(name) {
            return Err(update_error(format!(
                "release checksum manifest contains duplicate {name} entries"
            )));
        }
        if name == filename {
            selected = Some(checksum.to_owned());
        }
    }
    selected.ok_or_else(|| {
        update_error(format!(
            "release checksum manifest has no entry for {filename}"
        ))
    })
}

fn valid_checksum_entry(checksum: &str, name: &str) -> bool {
    checksum.len() == 64
        && checksum
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn verify_checksum(
    filename: &str,
    contents: &[u8],
    expected: &str,
) -> Result<(), NativeV2CliError> {
    let actual = format!("{:x}", Sha256::digest(contents));
    if actual != expected {
        return Err(update_error(format!(
            "release checksum does not match {filename}"
        )));
    }
    Ok(())
}

fn extract_executables(
    archive: &[u8],
    expected: &[&str],
    version: ReleaseVersion,
) -> Result<std::collections::BTreeMap<String, Vec<u8>>, NativeV2CliError> {
    extract_archive(archive, expected).map_err(|error| with_npm_hint(error, Some(version)))
}

fn extract_archive(
    archive: &[u8],
    expected: &[&str],
) -> Result<std::collections::BTreeMap<String, Vec<u8>>, NativeV2CliError> {
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    let decoder = GzDecoder::new(archive);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| update_error(format!("release archive is invalid: {error}")))?;
    let mut binaries = std::collections::BTreeMap::new();
    for entry in entries {
        insert_archive_entry(entry, &expected, &mut binaries)?;
    }
    if binaries.len() != expected.len() {
        return Err(update_error("release archive omits a required executable"));
    }
    Ok(binaries)
}

fn insert_archive_entry<R: Read>(
    entry: Result<tar::Entry<'_, R>, std::io::Error>,
    expected: &BTreeSet<&str>,
    binaries: &mut std::collections::BTreeMap<String, Vec<u8>>,
) -> Result<(), NativeV2CliError> {
    let mut entry =
        entry.map_err(|error| update_error(format!("release archive is invalid: {error}")))?;
    let path = entry
        .path()
        .map_err(|error| update_error(format!("release archive path is invalid: {error}")))?;
    let display = path.display().to_string();
    let Some(name) = path
        .to_str()
        .filter(|name| expected.contains(*name))
        .map(str::to_owned)
    else {
        return Err(update_error(format!(
            "release archive contains unexpected entry {display}"
        )));
    };
    if !entry.header().entry_type().is_file() {
        return Err(update_error(format!(
            "release archive contains non-file entry {display}"
        )));
    }
    if binaries.contains_key(&name) {
        return Err(update_error(format!(
            "release archive contains duplicate {name} entries"
        )));
    }
    if entry.size() > MAX_ARCHIVE_BYTES as u64 {
        return Err(update_error("release executable is too large"));
    }
    let mut contents = Vec::with_capacity(entry.size() as usize);
    entry
        .read_to_end(&mut contents)
        .map_err(|error| update_error(format!("could not extract release executable: {error}")))?;
    binaries.insert(name, contents);
    Ok(())
}

/// Installs `executables` over the executable this process runs. Returns whether it replaced it.
fn install(
    current: &Path,
    executables: &ReleaseExecutables,
    version: ReleaseVersion,
) -> Result<bool, NativeV2CliError> {
    install_at(
        current,
        executables,
        version,
        InstallOperations {
            verify: smoke,
            verify_restic: smoke_restic,
            replace: |staged: &Path| {
                self_replace::self_replace(staged).map_err(|error| {
                    update_error(format!("could not replace this executable: {error}"))
                })
            },
        },
    )
    .map_err(|error| with_npm_hint(error, Some(version)))
}

/// Probe the directory before downloading any assets, without touching the installed pair.
/// An already-current executable needs no replacement, even in a read-only installation.
fn check_installation(
    current: &Path,
    current_version: ReleaseVersion,
    latest: ReleaseVersion,
) -> Result<(), NativeV2CliError> {
    if latest <= current_version {
        return Ok(());
    }
    let current = installed_path(current);
    let parent = current
        .parent()
        .ok_or_else(|| update_error("this executable has no parent directory"))?;
    // An unnamed probe cannot race with another updater's staged-file cleanup.
    tempfile::tempfile_in(parent).map(drop).map_err(|error| {
        with_npm_hint(
            update_error(format!(
                "cannot replace this installation in {}: {error}",
                parent.display()
            )),
            Some(latest),
        )
    })
}

struct InstallOperations<Verify, VerifyRestic, Replace> {
    verify: Verify,
    verify_restic: VerifyRestic,
    replace: Replace,
}

/// Installs `executables` over `current`, the path this process reports for its executable, and
/// over its Restic sidecar, while holding the installation's update lock. Returns `false` without
/// replacing anything when the installation already holds exactly these executables, as it does
/// after an update that finished while this one waited installed the same release.
fn install_at<Verify, VerifyRestic, Replace>(
    current: &Path,
    executables: &ReleaseExecutables,
    version: ReleaseVersion,
    operations: InstallOperations<Verify, VerifyRestic, Replace>,
) -> Result<bool, NativeV2CliError>
where
    Verify: FnOnce(&Path, ReleaseVersion) -> Result<(), NativeV2CliError>,
    VerifyRestic: FnOnce(&Path) -> Result<(), NativeV2CliError>,
    Replace: FnOnce(&Path) -> Result<(), NativeV2CliError>,
{
    let current = installed_path(current);
    let parent = current
        .parent()
        .ok_or_else(|| update_error("this executable has no parent directory"))?;
    let restic = parent.join(executables.restic_name);
    let _lock = lock_installation(parent)?;
    remove_interrupted_updates(parent, &current, executables.restic_name);
    if holds_exactly(&current, &executables.zeroshot) && holds_exactly(&restic, &executables.restic)
    {
        return Ok(false);
    }
    let staged = stage_executable(
        parent,
        STAGED_ZEROSHOT_PREFIX,
        std::env::consts::EXE_SUFFIX,
        &executables.zeroshot,
    )?;
    let staged_restic = stage_executable(
        parent,
        STAGED_RESTIC_PREFIX,
        std::env::consts::EXE_SUFFIX,
        &executables.restic,
    )?;
    (operations.verify)(&staged, version)?;
    (operations.verify_restic)(&staged_restic)?;
    let sidecar = SidecarReplacement::install(&staged_restic, &restic)?;
    if let Err(error) = (operations.replace)(&staged) {
        sidecar.rollback()?;
        return Err(error);
    }
    sidecar.commit();
    Ok(true)
}

/// The installed path of the executable a process reports as `current`. Once another update has
/// replaced the executable, Linux reports the former path followed by ` (deleted)`, and the
/// installation is still at that former path.
fn installed_path(current: &Path) -> std::path::PathBuf {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt as _;

        if let Some(installed) = current.as_os_str().as_bytes().strip_suffix(b" (deleted)") {
            return std::ffi::OsStr::from_bytes(installed).into();
        }
    }
    current.to_owned()
}

/// Takes the exclusive update lock of the installation in `parent`, waiting while another update
/// holds it. The lock ends when the returned file closes, which the operating system also does
/// when its holder dies, so an interrupted update never blocks later ones.
fn lock_installation(parent: &Path) -> Result<std::fs::File, NativeV2CliError> {
    let path = parent.join(UPDATE_LOCK);
    let lock_error =
        |error: std::io::Error| update_error(format!("could not lock this installation: {error}"));
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .or_else(|error| {
            // Another user sharing the installation may own the lock file. Locking only reads it.
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                std::fs::File::open(&path).map_err(|_| error)
            } else {
                Err(error)
            }
        })
        .map_err(lock_error)?;
    fs2::FileExt::lock_exclusive(&lock).map_err(lock_error)?;
    Ok(lock)
}

/// Removes what interrupted updates left in `parent`: the executables they staged, the copy
/// `self_replace` makes before renaming it over `current`, and Restic sidecars they moved aside.
/// Updates create these files only while holding the installation lock, so under that lock each
/// one is abandoned. A missing sidecar means an update stopped after moving it aside and before
/// installing its own, so the newest backup is restored instead. Files that cannot be removed,
/// such as ones still in use on Windows, are left for a later update.
fn remove_interrupted_updates(parent: &Path, current: &Path, restic_name: &str) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    let backup_prefix = sidecar_backup_prefix(restic_name);
    // self_replace 1.5.0 names its copy `.zeroshot.__temp__*` on Unix and
    // `.zeroshot.*.__temp__.exe` on Windows.
    let replacement_prefix = current
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| format!(".{stem}."));
    let mut backups = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&backup_prefix) {
            backups.push(entry.path());
        } else if name.starts_with(STAGED_ZEROSHOT_PREFIX)
            || name.starts_with(STAGED_RESTIC_PREFIX)
            || replacement_prefix.as_deref().is_some_and(|prefix| {
                name.strip_prefix(prefix)
                    .is_some_and(|rest| rest.contains("__temp__"))
            })
        {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let sidecar = parent.join(restic_name);
    let Ok(sidecar_installed) = sidecar.try_exists() else {
        return;
    };
    // Backup names end in a UUIDv7, so they sort in the order they were made.
    backups.sort();
    if !sidecar_installed && let Some(newest) = backups.pop() {
        let _ = std::fs::rename(newest, sidecar);
    }
    for backup in backups {
        let _ = std::fs::remove_file(backup);
    }
}

/// Whether the file at `path` holds exactly `contents`.
fn holds_exactly(path: &Path, contents: &[u8]) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == contents.len() as u64)
        && std::fs::read(path).is_ok_and(|installed| installed == contents)
}

fn stage_executable(
    parent: &Path,
    prefix: &str,
    suffix: &str,
    contents: &[u8],
) -> Result<tempfile::TempPath, NativeV2CliError> {
    let mut staged = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .tempfile_in(parent)
        .map_err(|error| update_error(format!("could not stage the update: {error}")))?;
    staged
        .write_all(contents)
        .map_err(|error| update_error(format!("could not stage the update: {error}")))?;
    make_executable(staged.path())?;
    staged
        .as_file()
        .sync_all()
        .map_err(|error| update_error(format!("could not stage the update: {error}")))?;
    Ok(staged.into_temp_path())
}

/// A Restic sidecar installed in place of the previous one, which stays aside until the update
/// commits or rolls back. The installation lock keeps other updates away in the meantime, so a
/// rollback only ever removes the sidecar this update installed.
struct SidecarReplacement {
    destination: std::path::PathBuf,
    backup: Option<std::path::PathBuf>,
}

impl SidecarReplacement {
    fn install(staged: &Path, destination: &Path) -> Result<Self, NativeV2CliError> {
        let backup = destination.exists().then(|| {
            destination.with_file_name(format!(
                "{}{}",
                sidecar_backup_prefix(
                    destination
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("restic")
                ),
                uuid::Uuid::now_v7()
            ))
        });
        if let Some(backup) = &backup {
            std::fs::rename(destination, backup).map_err(|error| {
                update_error(format!(
                    "could not preserve the installed Restic sidecar: {error}"
                ))
            })?;
        }
        if let Err(error) = std::fs::rename(staged, destination) {
            if let Some(backup) = &backup {
                let _ = std::fs::rename(backup, destination);
            }
            return Err(update_error(format!(
                "could not install the Restic sidecar: {error}"
            )));
        }
        Ok(Self {
            destination: destination.to_owned(),
            backup,
        })
    }

    fn rollback(self) -> Result<(), NativeV2CliError> {
        std::fs::remove_file(&self.destination).map_err(|error| {
            update_error(format!("could not roll back the Restic sidecar: {error}"))
        })?;
        if let Some(backup) = self.backup {
            std::fs::rename(backup, self.destination).map_err(|error| {
                update_error(format!("could not restore the Restic sidecar: {error}"))
            })?;
        }
        Ok(())
    }

    fn commit(self) {
        if let Some(backup) = self.backup {
            let _ = std::fs::remove_file(backup);
        }
    }
}

/// Name prefix of the backups an update makes of the installed sidecar `name`.
fn sidecar_backup_prefix(name: &str) -> String {
    format!(".{name}-update-backup-")
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), NativeV2CliError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| update_error(format!("could not prepare the update: {error}")))
}

#[cfg(windows)]
fn make_executable(_path: &Path) -> Result<(), NativeV2CliError> {
    Ok(())
}

fn smoke(path: &Path, version: ReleaseVersion) -> Result<(), NativeV2CliError> {
    let result = Command::new(path)
        .arg("--version")
        .output()
        .map_err(|error| update_error(format!("could not verify the update: {error}")))?;
    validate_smoke_result(result.status.success(), &result.stdout, version)
}

fn smoke_restic(path: &Path) -> Result<(), NativeV2CliError> {
    let result = Command::new(path)
        .arg("version")
        .output()
        .map_err(|error| update_error(format!("could not verify Restic: {error}")))?;
    let stdout = std::str::from_utf8(&result.stdout).unwrap_or_default();
    if !result.status.success()
        || !stdout.starts_with(&format!("restic {RESTIC_VERSION} compiled with "))
    {
        return Err(update_error(
            "downloaded Restic executable failed verification",
        ));
    }
    Ok(())
}

fn validate_smoke_result(
    success: bool,
    stdout: &[u8],
    version: ReleaseVersion,
) -> Result<(), NativeV2CliError> {
    let expected = format!("zeroshot {version}\n");
    if !success || stdout != expected.as_bytes() {
        return Err(update_error(
            "downloaded release executable failed verification",
        ));
    }
    Ok(())
}

fn write_result(output: &mut impl Write, result: UpdateResult) -> Result<(), NativeV2CliError> {
    serde_json::to_writer(&mut *output, &result)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn update_error(message: impl Into<String>) -> NativeV2CliError {
    NativeV2CliError::Update(message.into())
}

/// A failed lookup has no resolved version; npm can resolve its own latest release.
fn npm_install_command(version: Option<ReleaseVersion>) -> String {
    let version = version.map_or_else(|| "latest".to_owned(), |version| version.to_string());
    format!("npm install --global @the-open-engine-company/zeroshot@{version}")
}

fn with_npm_hint(error: NativeV2CliError, version: Option<ReleaseVersion>) -> NativeV2CliError {
    match error {
        NativeV2CliError::Update(message) => update_error(format!(
            "{message}; reinstall with: {}",
            npm_install_command(version)
        )),
        error => error,
    }
}

#[cfg(test)]
#[path = "update/tests.rs"]
mod tests;
