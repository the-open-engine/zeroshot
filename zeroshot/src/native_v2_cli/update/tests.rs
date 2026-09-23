use std::cell::Cell;
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::thread::JoinHandle;

use flate2::{Compression, write::GzEncoder};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

#[test]
fn wave8_cli_contract_release_versions_are_canonical_and_ordered() {
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
fn wave8_cli_contract_checksum_manifest_accepts_exact_release_names_and_digests() {
    let binary = b"release binary";
    let filename = "zeroshot-v8.2.1-x86_64-unknown-linux-musl.tar.gz";
    let checksum = format!("{:x}", Sha256::digest(binary));
    let skill = b"canonical skill";
    let skill_checksum = format!("{:x}", Sha256::digest(skill));
    let manifest = format!("{checksum}  {filename}\n{skill_checksum}  {SKILL_ASSET}\n");

    let expected = checksum_for(manifest.as_bytes(), filename).assert_value();
    verify_checksum(filename, binary, &expected).assert_value();
    let expected_skill = checksum_for(manifest.as_bytes(), SKILL_ASSET).assert_value();
    verify_checksum(SKILL_ASSET, skill, &expected_skill).assert_value();
    assert!(verify_checksum(filename, b"tampered", &expected).is_err());
}

#[test]
fn wave8_cli_contract_checksum_manifest_rejects_invalid_missing_and_duplicate_entries() {
    let filename = "zeroshot-v8.2.1-x86_64-unknown-linux-musl.tar.gz";
    let digest = "0".repeat(64);
    let missing = format!("{digest}  {SKILL_ASSET}\n");
    assert!(
        checksum_for(missing.as_bytes(), filename)
            .assert_error()
            .to_string()
            .contains("no entry")
    );
    let duplicate = format!("{digest}  {filename}\n{digest}  {filename}\n");
    assert!(
        checksum_for(duplicate.as_bytes(), filename)
            .assert_error()
            .to_string()
            .contains("duplicate")
    );
    let duplicate_other = format!("{digest}  other\n{digest}  other\n");
    assert!(checksum_for(duplicate_other.as_bytes(), filename).is_err());

    let invalid_utf8 = [0xff];
    assert!(
        checksum_for(&invalid_utf8, filename)
            .assert_error()
            .to_string()
            .contains("not UTF-8")
    );
    for invalid in [
        format!("{}  {filename}\n", "0".repeat(63)),
        format!("{}  {filename}\n", "A".repeat(64)),
        format!("{digest} *{filename}\n"),
        format!("{digest}  ../{filename}\n"),
        format!("{digest}  nested/{filename}\n"),
        format!("{digest}  name with spaces\n"),
        format!("{digest}  \n"),
    ] {
        assert!(
            checksum_for(invalid.as_bytes(), filename)
                .assert_error()
                .to_string()
                .contains("invalid line"),
            "accepted invalid manifest line {invalid:?}"
        );
    }
}

#[test]
fn wave8_cli_contract_release_archive_accepts_one_exact_regular_executable() {
    let binary = b"release binary";
    let archive = test_archive(&[ArchiveEntry::File("zeroshot", binary)]);
    assert_eq!(
        extract_executable(&archive, "zeroshot").assert_value(),
        binary
    );
}

#[test]
fn wave8_cli_contract_release_archive_rejects_wrong_paths_types_duplicates_and_missing_executables()
{
    let cases = [
        (
            test_archive(&[ArchiveEntry::File("bin/zeroshot", b"binary")]),
            "unexpected entry",
        ),
        (
            test_archive(&[ArchiveEntry::Directory("zeroshot")]),
            "unexpected entry",
        ),
        (
            test_archive(&[
                ArchiveEntry::File("zeroshot", b"first"),
                ArchiveEntry::File("zeroshot", b"second"),
            ]),
            "duplicate",
        ),
        (test_archive(&[]), "does not contain"),
    ];
    for (archive, expected) in cases {
        let error = extract_executable(&archive, "zeroshot")
            .assert_error()
            .to_string();
        assert!(error.contains(expected), "{error}");
    }
}

#[tokio::test]
async fn bounded_download_accepts_success_and_rejects_status_and_both_oversize_forms() {
    let server = TestServer::start(vec![
        fixed_response("200 OK", b"data"),
        fixed_response("503 Service Unavailable", b"unavailable"),
        fixed_response("200 OK", b"large"),
        chunked_response(&[b"abc", b"def"]),
    ]);
    let client = http_client().assert_value();
    let success = download(&client, &server.url("success"), 4, "success").await;
    let status = download(&client, &server.url("status"), 64, "status").await;
    let declared = download(&client, &server.url("declared"), 4, "declared").await;
    let chunked = download(&client, &server.url("chunked"), 5, "chunked").await;
    server.finish();

    assert_eq!(success.assert_value(), b"data");
    assert!(status.assert_error().to_string().contains("HTTP 503"));
    assert!(
        declared
            .assert_error()
            .to_string()
            .contains("declared exceeds 4 bytes")
    );
    assert!(
        chunked
            .assert_error()
            .to_string()
            .contains("chunked exceeds 5 bytes")
    );
}

#[tokio::test]
async fn wave5_cli_contract_release_metadata_and_verified_assets_are_self_consistent() {
    let metadata = TestServer::start(vec![
        fixed_response("200 OK", br#"{"tag_name":"v8.2.1"}"#),
        fixed_response("200 OK", br#"{"tag_name":"8.2.1"}"#),
        fixed_response("200 OK", br#"{"tag_name":"v08.2.1"}"#),
        fixed_response("200 OK", br#"{"tag_name":"v7.9.9"}"#),
        fixed_response("200 OK", b"not json"),
    ]);
    let client = http_client().assert_value();
    assert_eq!(
        latest_version_at(&client, &metadata.url("canonical"))
            .await
            .assert_value(),
        ReleaseVersion([8, 2, 1])
    );
    for path in ["untagged", "leading-zero", "retired-major", "malformed"] {
        assert!(
            latest_version_at(&client, &metadata.url(path))
                .await
                .is_err()
        );
    }
    metadata.finish();

    let latest = ReleaseVersion([8, 2, 1]);
    let target = release_target().assert_value().0;
    let filename = format!("zeroshot-v{latest}-{target}.tar.gz");
    let binary = b"verified release binary";
    let archive = test_archive(&[ArchiveEntry::File(
        release_target().assert_value().1,
        binary,
    )]);
    let canonical_skill = b"---\nname: zeroshot\ndescription: Release skill\n---\n\nVerified.\n";
    let manifest = format!(
        "{:x}  {SKILL_ASSET}\n{:x}  {filename}\n",
        Sha256::digest(canonical_skill),
        Sha256::digest(&archive),
    );
    let assets = TestServer::start(vec![
        fixed_response("200 OK", manifest.as_bytes()),
        fixed_response("200 OK", canonical_skill),
        fixed_response("200 OK", &archive),
    ]);
    let release = download_release_at(&client, ReleaseVersion([8, 2, 0]), latest, &assets.base_url)
        .await
        .assert_value();
    assert_eq!(release.binary.assert_value(), binary);
    assets.finish();

    let unchanged = TestServer::start(vec![
        fixed_response("200 OK", manifest.as_bytes()),
        fixed_response("200 OK", canonical_skill),
    ]);
    assert!(
        download_release_at(&client, latest, latest, &unchanged.base_url)
            .await
            .assert_value()
            .binary
            .is_none()
    );
    unchanged.finish();

    assert_eq!(
        apply_release(
            &client,
            ReleaseVersion([8, 2, 1]),
            ReleaseVersion([8, 2, 0]),
        )
        .await
        .assert_value(),
        (false, false)
    );
    assert!(installed_version().is_err());
}

#[test]
fn wave8_cli_contract_installation_stages_verifies_replaces_and_cleans_up() {
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
fn wave8_cli_contract_installation_stops_on_verification_or_replacement_failure() {
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
fn wave8_cli_contract_smoke_and_result_reporting_require_the_exact_release_version() {
    let version = ReleaseVersion([8, 2, 1]);
    validate_smoke_result(true, b"zeroshot 8.2.1\n", version).assert_value();
    assert!(validate_smoke_result(false, b"zeroshot 8.2.1\n", version).is_err());
    assert!(validate_smoke_result(true, b"zeroshot 8.2.0\n", version).is_err());

    let mut output = Vec::new();
    write_result(
        &mut output,
        UpdateResult {
            current_version: ReleaseVersion([8, 2, 0]).to_string(),
            latest_version: version.to_string(),
            updated: true,
            skill_updated: true,
        },
    )
    .assert_value();
    assert_eq!(
        output,
        br#"{"currentVersion":"8.2.0","latestVersion":"8.2.1","updated":true,"skillUpdated":true}
"#
    );
}

#[test]
fn wave8_cli_contract_update_process_boundaries_fail_before_replacement() {
    fn verify_noop(_: &Path, _: ReleaseVersion) -> Result<(), NativeV2CliError> {
        Ok(())
    }

    fn replace_noop(_: &Path) -> Result<(), NativeV2CliError> {
        Ok(())
    }

    let version = ReleaseVersion([8, 2, 1]);
    let (target, executable) = release_target().assert_value();
    assert!(!target.is_empty());
    assert!(matches!(executable, "zeroshot" | "zeroshot.exe"));
    assert!(extract_executable(b"not a gzip archive", executable).is_err());
    assert!(
        install_at(
            Path::new(""),
            b"binary",
            version,
            InstallOperations {
                verify: verify_noop,
                replace: replace_noop,
            },
        )
        .is_err()
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().assert_value();
        let candidate = directory.path().join("zeroshot");
        fs::write(&candidate, b"#!/bin/sh\nprintf 'zeroshot 8.2.1\\n'\n").assert_value();
        fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755)).assert_value();
        smoke(&candidate, version).assert_value();
        assert!(smoke(&candidate, ReleaseVersion([8, 2, 2])).is_err());
        let missing = directory.path().join("missing");
        assert!(make_executable(&missing).is_err());
        assert!(smoke(&missing, version).is_err());
    }
}

enum ArchiveEntry<'a> {
    File(&'a str, &'a [u8]),
    Directory(&'a str),
}

fn test_archive(entries: &[ArchiveEntry<'_>]) -> Vec<u8> {
    let encoder = GzEncoder::new(Vec::new(), Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for entry in entries {
        match entry {
            ArchiveEntry::File(name, contents) => {
                let mut header = tar::Header::new_gnu();
                header.set_size(contents.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                archive.append_data(&mut header, name, *contents).unwrap();
            }
            ArchiveEntry::Directory(name) => {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_mode(0o755);
                header.set_cksum();
                archive
                    .append_data(&mut header, name, std::io::empty())
                    .unwrap();
            }
        }
    }
    archive.into_inner().unwrap().finish().unwrap()
}

struct TestServer {
    base_url: String,
    thread: JoinHandle<()>,
}

impl TestServer {
    fn start(responses: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let mut chunk = [0; 512];
                    let read = stream.read(&mut chunk).unwrap();
                    assert!(read > 0, "client closed before sending request headers");
                    request.extend_from_slice(&chunk[..read]);
                    assert!(
                        request.len() <= 8 * 1024,
                        "request headers are unexpectedly large"
                    );
                }
                stream.write_all(&response).unwrap();
                stream.flush().unwrap();
            }
        });
        Self {
            base_url: format!("http://{address}"),
            thread,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base_url)
    }

    fn finish(self) {
        self.thread.join().unwrap();
    }
}

fn fixed_response(status: &str, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

fn chunked_response(chunks: &[&[u8]]) -> Vec<u8> {
    let mut response =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n".to_vec();
    for chunk in chunks {
        response.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        response.extend_from_slice(chunk);
        response.extend_from_slice(b"\r\n");
    }
    response.extend_from_slice(b"0\r\n\r\n");
    response
}
