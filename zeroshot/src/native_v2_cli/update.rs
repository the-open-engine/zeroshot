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

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/the-open-engine/zeroshot/releases/latest";
const RELEASE_BASE_URL: &str = "https://github.com/the-open-engine/zeroshot/releases/download";
const MINIMUM_RELEASE_MAJOR: u64 = 8;
const MAX_RELEASE_BYTES: usize = 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_SKILL_BYTES: usize = 1024 * 1024;
const MAX_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;
const SKILL_ASSET: &str = "zeroshot-skill.md";
const RESTIC_VERSION: &str = "0.19.1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

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
    skill: skill::PreparedSkill,
}

struct ReleaseExecutables {
    zeroshot: Vec<u8>,
    restic: Vec<u8>,
    restic_name: &'static str,
}

pub(super) async fn execute(output: &mut impl Write) -> Result<CliOutcome, NativeV2CliError> {
    let current = installed_version()?;
    let client = http_client()?;
    let latest = latest_version(&client).await?;
    let (updated, skill_updated) = apply_release(&client, current, latest).await?;
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
    current: ReleaseVersion,
    latest: ReleaseVersion,
) -> Result<(bool, bool), NativeV2CliError> {
    if latest < current {
        return Ok((false, false));
    }
    let release = download_release(client, current, latest).await?;
    let updated = release.executables.is_some();
    if let Some(executables) = release.executables {
        install(&executables, latest)?;
    }
    let skill_updated = release.skill.install()?;
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
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent("zeroshot self-update")
        .build()
        .map_err(|error| update_error(format!("could not initialize HTTPS: {error}")))
}

async fn latest_version(client: &reqwest::Client) -> Result<ReleaseVersion, NativeV2CliError> {
    latest_version_at(client, LATEST_RELEASE_URL).await
}

async fn latest_version_at(
    client: &reqwest::Client,
    url: &str,
) -> Result<ReleaseVersion, NativeV2CliError> {
    let metadata = download(client, url, MAX_RELEASE_BYTES, "latest release").await?;
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

async fn download_release(
    client: &reqwest::Client,
    current: ReleaseVersion,
    latest: ReleaseVersion,
) -> Result<ReleaseUpdate, NativeV2CliError> {
    let base_url = format!("{RELEASE_BASE_URL}/v{latest}");
    download_release_at(client, current, latest, &base_url).await
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
    let skill_contents = release.download(SKILL_ASSET, MAX_SKILL_BYTES).await?;
    let skill = skill::prepare(skill_contents)?;
    let executables = if latest > current {
        let (target, executable, restic) = release_target()?;
        let filename = format!("zeroshot-v{latest}-{target}.tar.gz");
        let archive = release.download(&filename, MAX_ARCHIVE_BYTES).await?;
        let mut extracted = extract_executables(&archive, &[executable, restic])?;
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
    Ok(ReleaseUpdate { executables, skill })
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
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok(("x86_64-unknown-linux-musl", "zeroshot", "restic")),
        ("linux", "aarch64") => Ok(("aarch64-unknown-linux-musl", "zeroshot", "restic")),
        ("macos", "x86_64") => Ok(("x86_64-apple-darwin", "zeroshot", "restic")),
        ("macos", "aarch64") => Ok(("aarch64-apple-darwin", "zeroshot", "restic")),
        ("windows", "x86_64") => Ok(("x86_64-pc-windows-msvc", "zeroshot.exe", "restic.exe")),
        (os, arch) => Err(update_error(format!(
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
    let response = client.get(url).send().await.map_err(|error| {
        update_error(format!("could not fetch {label}: {}", error.without_url()))
    })?;
    if !response.status().is_success() {
        return Err(update_error(format!(
            "could not fetch {label}: HTTP {}",
            response.status()
        )));
    }
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(update_error(format!("{label} exceeds {maximum} bytes")));
    }
    let mut contents = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            update_error(format!("could not read {label}: {}", error.without_url()))
        })?;
        if contents.len().saturating_add(chunk.len()) > maximum {
            return Err(update_error(format!("{label} exceeds {maximum} bytes")));
        }
        contents.extend_from_slice(&chunk);
    }
    Ok(contents)
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

fn install(
    executables: &ReleaseExecutables,
    version: ReleaseVersion,
) -> Result<(), NativeV2CliError> {
    let current = std::env::current_exe()
        .map_err(|error| update_error(format!("could not locate this executable: {error}")))?;
    install_at(
        &current,
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
}

struct InstallOperations<Verify, VerifyRestic, Replace> {
    verify: Verify,
    verify_restic: VerifyRestic,
    replace: Replace,
}

fn install_at<Verify, VerifyRestic, Replace>(
    current: &Path,
    executables: &ReleaseExecutables,
    version: ReleaseVersion,
    operations: InstallOperations<Verify, VerifyRestic, Replace>,
) -> Result<(), NativeV2CliError>
where
    Verify: FnOnce(&Path, ReleaseVersion) -> Result<(), NativeV2CliError>,
    VerifyRestic: FnOnce(&Path) -> Result<(), NativeV2CliError>,
    Replace: FnOnce(&Path) -> Result<(), NativeV2CliError>,
{
    let parent = current
        .parent()
        .ok_or_else(|| update_error("this executable has no parent directory"))?;
    let staged = stage_executable(
        parent,
        ".zeroshot-update-",
        std::env::consts::EXE_SUFFIX,
        &executables.zeroshot,
    )?;
    let staged_restic = stage_executable(
        parent,
        ".restic-update-",
        std::env::consts::EXE_SUFFIX,
        &executables.restic,
    )?;
    (operations.verify)(&staged, version)?;
    (operations.verify_restic)(&staged_restic)?;
    let sidecar =
        SidecarReplacement::install(&staged_restic, &parent.join(executables.restic_name))?;
    if let Err(error) = (operations.replace)(&staged) {
        sidecar.rollback()?;
        return Err(error);
    }
    sidecar.commit();
    Ok(())
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

struct SidecarReplacement {
    destination: std::path::PathBuf,
    backup: Option<std::path::PathBuf>,
}

impl SidecarReplacement {
    fn install(staged: &Path, destination: &Path) -> Result<Self, NativeV2CliError> {
        let backup = destination.exists().then(|| {
            destination.with_file_name(format!(
                ".{}-update-backup-{}",
                destination
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("restic"),
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

#[cfg(test)]
#[path = "update/tests.rs"]
mod tests;
