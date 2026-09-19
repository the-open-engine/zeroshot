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

use super::{CliOutcome, NativeV2CliError};

const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/the-open-engine/zeroshot/releases/latest";
const RELEASE_BASE_URL: &str = "https://github.com/the-open-engine/zeroshot/releases/download";
const MINIMUM_RELEASE_MAJOR: u64 = 8;
const MAX_RELEASE_BYTES: usize = 1024 * 1024;
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const MAX_ARCHIVE_BYTES: usize = 256 * 1024 * 1024;
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
}

pub(super) async fn execute(output: &mut impl Write) -> Result<CliOutcome, NativeV2CliError> {
    let current = installed_version()?;
    let client = http_client()?;
    let latest = latest_version(&client).await?;
    if latest > current {
        download_and_install(&client, latest).await?;
    }
    write_result(output, current, latest, latest > current)?;
    Ok(CliOutcome::Completed)
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
    let metadata = download(
        client,
        LATEST_RELEASE_URL,
        MAX_RELEASE_BYTES,
        "latest release",
    )
    .await?;
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

async fn download_and_install(
    client: &reqwest::Client,
    latest: ReleaseVersion,
) -> Result<(), NativeV2CliError> {
    let (target, executable) = release_target()?;
    let filename = format!("zeroshot-v{latest}-{target}.tar.gz");
    let base_url = format!("{RELEASE_BASE_URL}/v{latest}");
    let manifest = download(
        client,
        &format!("{base_url}/SHA256SUMS"),
        MAX_MANIFEST_BYTES,
        "release checksum manifest",
    )
    .await?;
    let expected_checksum = checksum_for(&manifest, &filename)?;
    let archive = download(
        client,
        &format!("{base_url}/{filename}"),
        MAX_ARCHIVE_BYTES,
        "release archive",
    )
    .await?;
    verify_checksum(&filename, &archive, &expected_checksum)?;
    let binary = extract_executable(&archive, executable)?;
    install(&binary, latest)
}

fn release_version(value: &str, kind: &str) -> Result<ReleaseVersion, NativeV2CliError> {
    let version = ReleaseVersion::parse(value)
        .filter(|version| version.to_string() == value)
        .ok_or_else(|| update_error(format!("{kind} {value:?} is not canonical")))?;
    Ok(version)
}

fn release_target() -> Result<(&'static str, &'static str), NativeV2CliError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok(("x86_64-unknown-linux-musl", "zeroshot")),
        ("linux", "aarch64") => Ok(("aarch64-unknown-linux-musl", "zeroshot")),
        ("macos", "x86_64") => Ok(("x86_64-apple-darwin", "zeroshot")),
        ("macos", "aarch64") => Ok(("aarch64-apple-darwin", "zeroshot")),
        ("windows", "x86_64") => Ok(("x86_64-pc-windows-msvc", "zeroshot.exe")),
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

fn extract_executable(archive: &[u8], executable: &str) -> Result<Vec<u8>, NativeV2CliError> {
    let decoder = GzDecoder::new(archive);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|error| update_error(format!("release archive is invalid: {error}")))?;
    let mut binary = None;
    for entry in entries {
        let mut entry =
            entry.map_err(|error| update_error(format!("release archive is invalid: {error}")))?;
        let path = entry
            .path()
            .map_err(|error| update_error(format!("release archive path is invalid: {error}")))?;
        if path.as_ref() != Path::new(executable) || !entry.header().entry_type().is_file() {
            return Err(update_error(format!(
                "release archive contains unexpected entry {}",
                path.display()
            )));
        }
        if binary.is_some() {
            return Err(update_error(format!(
                "release archive contains duplicate {executable} entries"
            )));
        }
        if entry.size() > MAX_ARCHIVE_BYTES as u64 {
            return Err(update_error("release executable is too large"));
        }
        let mut contents = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut contents).map_err(|error| {
            update_error(format!("could not extract release executable: {error}"))
        })?;
        binary = Some(contents);
    }
    binary.ok_or_else(|| update_error(format!("release archive does not contain {executable}")))
}

fn install(binary: &[u8], version: ReleaseVersion) -> Result<(), NativeV2CliError> {
    let current = std::env::current_exe()
        .map_err(|error| update_error(format!("could not locate this executable: {error}")))?;
    install_at(
        &current,
        binary,
        version,
        InstallOperations {
            verify: smoke,
            replace: |staged: &Path| {
                self_replace::self_replace(staged).map_err(|error| {
                    update_error(format!("could not replace this executable: {error}"))
                })
            },
        },
    )
}

struct InstallOperations<Verify, Replace> {
    verify: Verify,
    replace: Replace,
}

fn install_at<Verify, Replace>(
    current: &Path,
    binary: &[u8],
    version: ReleaseVersion,
    operations: InstallOperations<Verify, Replace>,
) -> Result<(), NativeV2CliError>
where
    Verify: FnOnce(&Path, ReleaseVersion) -> Result<(), NativeV2CliError>,
    Replace: FnOnce(&Path) -> Result<(), NativeV2CliError>,
{
    let parent = current
        .parent()
        .ok_or_else(|| update_error("this executable has no parent directory"))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".zeroshot-update-")
        .suffix(std::env::consts::EXE_SUFFIX)
        .tempfile_in(parent)
        .map_err(|error| update_error(format!("could not stage the update: {error}")))?;
    staged
        .write_all(binary)
        .map_err(|error| update_error(format!("could not stage the update: {error}")))?;
    make_executable(staged.path())?;
    staged
        .as_file()
        .sync_all()
        .map_err(|error| update_error(format!("could not stage the update: {error}")))?;
    let staged = staged.into_temp_path();
    (operations.verify)(&staged, version)?;
    (operations.replace)(&staged)
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

fn write_result(
    output: &mut impl Write,
    current: ReleaseVersion,
    latest: ReleaseVersion,
    updated: bool,
) -> Result<(), NativeV2CliError> {
    serde_json::to_writer(
        &mut *output,
        &UpdateResult {
            current_version: current.to_string(),
            latest_version: latest.to_string(),
            updated,
        },
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn update_error(message: impl Into<String>) -> NativeV2CliError {
    NativeV2CliError::Update(message.into())
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fs;

    use flate2::{Compression, write::GzEncoder};
    use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

    use super::*;

    #[test]
    fn release_versions_are_canonical_and_ordered() {
        assert_eq!(
            ReleaseVersion::parse("8.2.1").assert_value().to_string(),
            "8.2.1"
        );
        assert!(ReleaseVersion::parse("8.2").is_none());
        assert!(ReleaseVersion::parse("v8.2.1").is_none());
        assert!(ReleaseVersion::parse("8.2.beta").is_none());
        assert!(release_version("08.2.1", "test").is_err());
        assert!(
            ReleaseVersion::parse("8.10.0").assert_value()
                > ReleaseVersion::parse("8.9.9").assert_value()
        );
    }

    #[test]
    fn checksum_and_archive_validation_accept_the_release_shape() {
        let binary = b"release binary";
        let archive = test_archive(&[("zeroshot", binary)]);
        let filename = "zeroshot-v8.2.1-x86_64-unknown-linux-musl.tar.gz";
        let checksum = format!("{:x}", Sha256::digest(&archive));
        let manifest = format!("{checksum}  {filename}\n");

        let expected = checksum_for(manifest.as_bytes(), filename).assert_value();
        verify_checksum(filename, &archive, &expected).assert_value();
        assert_eq!(
            extract_executable(&archive, "zeroshot").assert_value(),
            binary
        );
    }

    #[test]
    fn checksum_and_archive_validation_fail_closed() {
        let filename = "zeroshot-v8.2.1-x86_64-unknown-linux-musl.tar.gz";
        let manifest = format!("{}  {filename}\n", "0".repeat(64));
        let expected = checksum_for(manifest.as_bytes(), filename).assert_value();
        assert!(matches!(
            verify_checksum(filename, b"tampered", &expected).assert_error(),
            NativeV2CliError::Update(_)
        ));

        let duplicate = format!("{0}  other\n{0}  other\n", "0".repeat(64));
        assert!(matches!(
            checksum_for(duplicate.as_bytes(), filename).assert_error(),
            NativeV2CliError::Update(_)
        ));

        let archive = test_archive(&[("other", b"binary")]);
        assert!(matches!(
            extract_executable(&archive, "zeroshot").assert_error(),
            NativeV2CliError::Update(_)
        ));
    }

    #[test]
    fn installation_stages_verifies_replaces_and_cleans_up() {
        let directory = tempfile::tempdir().unwrap();
        let current = directory
            .path()
            .join(format!("zeroshot{}", std::env::consts::EXE_SUFFIX));
        let installed = directory.path().join("installed");
        fs::write(&current, b"old binary").unwrap();
        let verified = Cell::new(false);
        let version = ReleaseVersion([8, 2, 1]);

        install_at(
            &current,
            b"new binary",
            version,
            InstallOperations {
                verify: |staged: &Path, actual_version| {
                    assert_eq!(staged.parent(), current.parent());
                    assert_eq!(actual_version, version);
                    assert_eq!(fs::read(staged).unwrap(), b"new binary");
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;

                        assert_eq!(
                            fs::metadata(staged).unwrap().permissions().mode() & 0o777,
                            0o755
                        );
                    }
                    verified.set(true);
                    Ok(())
                },
                replace: |staged: &Path| {
                    assert!(verified.get());
                    fs::copy(staged, &installed)
                        .map(|_| ())
                        .map_err(|error| update_error(format!("test replacement failed: {error}")))
                },
            },
        )
        .assert_value();

        assert_eq!(fs::read(installed).unwrap(), b"new binary");
        assert!(!directory.path().read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".zeroshot-update-")
        }));
    }

    #[test]
    fn installation_stops_on_verification_or_replacement_failure() {
        let directory = tempfile::tempdir().unwrap();
        let current = directory.path().join("zeroshot");
        fs::write(&current, b"old binary").unwrap();
        let replaced = Cell::new(false);
        let version = ReleaseVersion([8, 2, 1]);

        let verification_error = install_at(
            &current,
            b"new binary",
            version,
            InstallOperations {
                verify: |_: &Path, _| Err(update_error("verification failed")),
                replace: |_: &Path| {
                    replaced.set(true);
                    Ok(())
                },
            },
        )
        .assert_error();
        assert!(matches!(verification_error, NativeV2CliError::Update(_)));
        assert!(!replaced.get());

        let replacement_error = install_at(
            &current,
            b"new binary",
            version,
            InstallOperations {
                verify: |_: &Path, _| Ok(()),
                replace: |_: &Path| Err(update_error("replacement failed")),
            },
        )
        .assert_error();
        assert!(matches!(replacement_error, NativeV2CliError::Update(_)));
    }

    #[test]
    fn smoke_and_result_reporting_require_the_exact_release_version() {
        let version = ReleaseVersion([8, 2, 1]);
        validate_smoke_result(true, b"zeroshot 8.2.1\n", version).assert_value();
        assert!(validate_smoke_result(false, b"zeroshot 8.2.1\n", version).is_err());
        assert!(validate_smoke_result(true, b"zeroshot 8.2.0\n", version).is_err());

        let mut output = Vec::new();
        write_result(&mut output, ReleaseVersion([8, 2, 0]), version, true).assert_value();
        assert_eq!(
            output,
            br#"{"currentVersion":"8.2.0","latestVersion":"8.2.1","updated":true}
"#
        );
    }

    fn test_archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (name, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive.append_data(&mut header, name, *contents).unwrap();
        }
        archive.into_inner().unwrap().finish().unwrap()
    }
}
