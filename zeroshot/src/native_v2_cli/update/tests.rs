use std::cell::Cell;
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::thread::JoinHandle;

use flate2::{Compression, write::GzEncoder};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

fn assert_release_versions_are_canonical_and_ordered() {
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

fn assert_checksum_manifest_accepts_exact_release_names_and_digests() {
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

fn assert_checksum_manifest_rejects_invalid_missing_and_duplicate_entries() {
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

fn assert_release_archive_accepts_exact_regular_executables() {
    let binary = b"release binary";
    let restic = b"restic binary";
    let archive = test_archive(&[
        ArchiveEntry::File("zeroshot", binary),
        ArchiveEntry::File("restic", restic),
    ]);
    let extracted = extract_executables(&archive, &["zeroshot", "restic"]).assert_value();
    assert_eq!(extracted["zeroshot"], binary);
    assert_eq!(extracted["restic"], restic);
}

fn assert_release_archive_rejects_wrong_paths_types_duplicates_and_missing_executables() {
    let cases = [
        (
            test_archive(&[ArchiveEntry::File("bin/zeroshot", b"binary")]),
            "unexpected entry",
        ),
        (
            test_archive(&[ArchiveEntry::Directory("zeroshot")]),
            "non-file entry",
        ),
        (
            test_archive(&[
                ArchiveEntry::File("zeroshot", b"first"),
                ArchiveEntry::File("zeroshot", b"second"),
            ]),
            "duplicate",
        ),
        (test_archive(&[]), "omits a required executable"),
    ];
    for (archive, expected) in cases {
        let error = extract_executables(&archive, &["zeroshot", "restic"])
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
        truncated_response(8, b"short"),
    ]);
    let client = http_client().assert_value();
    let success = download(&client, &server.url("success"), 4, "success").await;
    let status = download(&client, &server.url("status"), 64, "status").await;
    let declared = download(&client, &server.url("declared"), 4, "declared").await;
    let chunked = download(&client, &server.url("chunked"), 5, "chunked").await;
    let truncated = download(&client, &server.url("truncated"), 8, "truncated").await;
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
    assert!(
        truncated
            .assert_error()
            .to_string()
            .contains("could not read truncated")
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
    let (target, executable, restic_name) = release_target().assert_value();
    let filename = format!("zeroshot-v{latest}-{target}.tar.gz");
    let binary = b"verified release binary";
    let restic = b"verified restic binary";
    let archive = test_archive(&[
        ArchiveEntry::File(executable, binary),
        ArchiveEntry::File(restic_name, restic),
    ]);
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
    let executables = release.executables.assert_value();
    assert_eq!(executables.zeroshot, binary);
    assert_eq!(executables.restic, restic);
    assert_eq!(executables.restic_name, restic_name);
    assets.finish();

    let unchanged = TestServer::start(vec![
        fixed_response("200 OK", manifest.as_bytes()),
        fixed_response("200 OK", canonical_skill),
    ]);
    assert!(
        download_release_at(&client, latest, latest, &unchanged.base_url)
            .await
            .assert_value()
            .executables
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

fn assert_installation_stages_verifies_replaces_and_cleans_up() {
    let directory = tempfile::tempdir().unwrap();
    let current = directory
        .path()
        .join(format!("zeroshot{}", std::env::consts::EXE_SUFFIX));
    let installed = directory.path().join("installed");
    fs::write(&current, b"old binary").unwrap();
    fs::write(directory.path().join("restic"), b"old restic").unwrap();
    let verified = Cell::new(false);
    let version = ReleaseVersion([8, 2, 1]);
    let executables = test_executables();

    install_at(
        &current,
        &executables,
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
            verify_restic: |staged: &Path| {
                assert_eq!(fs::read(staged).unwrap(), b"new restic");
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
    assert_eq!(
        fs::read(directory.path().join("restic")).unwrap(),
        b"new restic"
    );
    assert!(!directory.path().read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".zeroshot-update-")
    }));
    assert!(!directory.path().read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".restic-update-")
    }));
}

fn assert_installation_stops_on_verification_or_replacement_failure() {
    let directory = tempfile::tempdir().unwrap();
    let current = directory.path().join("zeroshot");
    fs::write(&current, b"old binary").unwrap();
    fs::write(directory.path().join("restic"), b"old restic").unwrap();
    let replaced = Cell::new(false);
    let version = ReleaseVersion([8, 2, 1]);
    let executables = test_executables();

    let verification_error = install_at(
        &current,
        &executables,
        version,
        InstallOperations {
            verify: |_: &Path, _| Err(update_error("verification failed")),
            verify_restic: |_: &Path| Ok(()),
            replace: |_: &Path| {
                replaced.set(true);
                Ok(())
            },
        },
    )
    .assert_error();
    assert!(matches!(verification_error, NativeV2CliError::Update(_)));
    assert!(!replaced.get());

    let restic_verification_error = install_at(
        &current,
        &executables,
        version,
        InstallOperations {
            verify: |_: &Path, _| Ok(()),
            verify_restic: |_: &Path| Err(update_error("Restic verification failed")),
            replace: |_: &Path| {
                replaced.set(true);
                Ok(())
            },
        },
    )
    .assert_error();
    assert!(matches!(
        restic_verification_error,
        NativeV2CliError::Update(_)
    ));
    assert!(!replaced.get());
    assert_eq!(
        fs::read(directory.path().join("restic")).unwrap(),
        b"old restic"
    );

    let replacement_error = install_at(
        &current,
        &executables,
        version,
        InstallOperations {
            verify: |_: &Path, _| Ok(()),
            verify_restic: |_: &Path| Ok(()),
            replace: |_: &Path| Err(update_error("replacement failed")),
        },
    )
    .assert_error();
    assert!(matches!(replacement_error, NativeV2CliError::Update(_)));
    assert_eq!(
        fs::read(directory.path().join("restic")).unwrap(),
        b"old restic"
    );

    let missing_staged = directory.path().join("missing-staged-restic");
    let sidecar_error =
        SidecarReplacement::install(&missing_staged, &directory.path().join("restic"))
            .assert_error()
            .to_string();
    assert!(sidecar_error.contains("could not install the Restic sidecar"));
    assert_eq!(
        fs::read(directory.path().join("restic")).unwrap(),
        b"old restic"
    );
    assert!(!directory.path().read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".restic-update-backup-")
    }));
}

fn assert_sidecar_recovery_failures_are_explicit_and_bounded() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    let rollback_error = SidecarReplacement {
        destination: missing.clone(),
        backup: None,
    }
    .rollback()
    .assert_error()
    .to_string();
    assert!(rollback_error.contains("could not roll back the Restic sidecar"));

    let installed = directory.path().join("installed-restic");
    fs::write(&installed, b"new restic").unwrap();
    let restore_error = SidecarReplacement {
        destination: installed.clone(),
        backup: Some(missing),
    }
    .rollback()
    .assert_error()
    .to_string();
    assert!(restore_error.contains("could not restore the Restic sidecar"));
    assert!(!installed.exists());

    let retained_backup = directory.path().join("retained-backup");
    fs::create_dir(&retained_backup).unwrap();
    fs::write(retained_backup.join("entry"), b"retained").unwrap();
    SidecarReplacement {
        destination: directory.path().join("unused"),
        backup: Some(retained_backup.clone()),
    }
    .commit();
    assert!(retained_backup.is_dir());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let locked = directory.path().join("locked");
        fs::create_dir(&locked).unwrap();
        let destination = locked.join("restic");
        fs::write(&destination, b"old restic").unwrap();
        let staged = directory.path().join("staged-restic");
        fs::write(&staged, b"new restic").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        let result = SidecarReplacement::install(&staged, &destination);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let error = result.assert_error().to_string();
        assert!(error.contains("could not preserve the installed Restic sidecar"));
        assert_eq!(fs::read(destination).unwrap(), b"old restic");
        assert_eq!(fs::read(staged).unwrap(), b"new restic");
    }
}

fn assert_smoke_and_result_reporting_require_the_exact_release_version() {
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

fn assert_update_process_boundaries_fail_before_replacement() {
    fn verify_noop(_: &Path, _: ReleaseVersion) -> Result<(), NativeV2CliError> {
        Ok(())
    }

    fn replace_noop(_: &Path) -> Result<(), NativeV2CliError> {
        Ok(())
    }

    let version = ReleaseVersion([8, 2, 1]);
    let (target, executable, restic) = release_target().assert_value();
    assert!(!target.is_empty());
    assert!(matches!(executable, "zeroshot" | "zeroshot.exe"));
    assert!(matches!(restic, "restic" | "restic.exe"));
    for (os, arch, expected) in [
        (
            "linux",
            "x86_64",
            ("x86_64-unknown-linux-musl", "zeroshot", "restic"),
        ),
        (
            "linux",
            "aarch64",
            ("aarch64-unknown-linux-musl", "zeroshot", "restic"),
        ),
        (
            "macos",
            "x86_64",
            ("x86_64-apple-darwin", "zeroshot", "restic"),
        ),
        (
            "macos",
            "aarch64",
            ("aarch64-apple-darwin", "zeroshot", "restic"),
        ),
        (
            "windows",
            "x86_64",
            ("x86_64-pc-windows-msvc", "zeroshot.exe", "restic.exe"),
        ),
    ] {
        assert_eq!(release_target_for(os, arch).assert_value(), expected);
    }
    let unsupported = release_target_for("plan9", "mips")
        .assert_error()
        .to_string();
    assert!(unsupported.contains("plan9/mips"));
    assert!(extract_executables(b"not a gzip archive", &[executable, restic]).is_err());
    assert!(
        install_at(
            Path::new(""),
            &test_executables(),
            version,
            InstallOperations {
                verify: verify_noop,
                verify_restic: |_: &Path| Ok(()),
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

        let restic = directory.path().join("restic");
        fs::write(
            &restic,
            b"#!/bin/sh\nprintf 'restic 0.19.1 compiled with go1.25\\n'\n",
        )
        .assert_value();
        fs::set_permissions(&restic, fs::Permissions::from_mode(0o755)).assert_value();
        smoke_restic(&restic).assert_value();
        for invalid in [
            b"#!/bin/sh\nprintf 'restic 0.19.0 compiled with go1.25\\n'\n".as_slice(),
            b"#!/bin/sh\nexit 1\n".as_slice(),
        ] {
            fs::write(&restic, invalid).assert_value();
            assert!(smoke_restic(&restic).is_err());
        }
        assert!(smoke_restic(&missing).is_err());

        let not_a_directory = directory.path().join("not-a-directory");
        fs::write(&not_a_directory, b"file").assert_value();
        assert!(stage_executable(&not_a_directory, ".test-", "", b"binary").is_err());
    }
}

async fn assert_update_short_circuits_and_serializes_without_release_io() {
    let current = ReleaseVersion([8, 4, 0]);
    let older = ReleaseVersion([8, 3, 9]);
    let client = http_client().assert_value();
    assert_eq!(
        apply_release(&client, current, older).await.assert_value(),
        (false, false)
    );
    assert!(installed_version().is_err());

    validate_smoke_result(true, b"zeroshot 8.4.0\n", current).assert_value();
    for (success, output) in [
        (false, b"zeroshot 8.4.0\n".as_slice()),
        (true, b"zeroshot 8.3.9\n".as_slice()),
        (true, b"zeroshot 8.4.0".as_slice()),
    ] {
        assert!(validate_smoke_result(success, output, current).is_err());
    }

    let mut output = Vec::new();
    write_result(
        &mut output,
        UpdateResult {
            current_version: current.to_string(),
            latest_version: current.to_string(),
            updated: false,
            skill_updated: true,
        },
    )
    .assert_value();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output).assert_value(),
        serde_json::json!({
            "currentVersion":"8.4.0",
            "latestVersion":"8.4.0",
            "updated":false,
            "skillUpdated":true
        })
    );
}

#[tokio::test]
async fn wave10_cli_contract_release_integrity_pipeline_is_lean_and_fail_closed() {
    assert_release_versions_are_canonical_and_ordered();
    assert_checksum_manifest_accepts_exact_release_names_and_digests();
    assert_checksum_manifest_rejects_invalid_missing_and_duplicate_entries();
    assert_release_archive_accepts_exact_regular_executables();
    assert_release_archive_rejects_wrong_paths_types_duplicates_and_missing_executables();
    assert_installation_stages_verifies_replaces_and_cleans_up();
    assert_installation_stops_on_verification_or_replacement_failure();
    assert_sidecar_recovery_failures_are_explicit_and_bounded();
    assert_smoke_and_result_reporting_require_the_exact_release_version();
    assert_update_process_boundaries_fail_before_replacement();
    assert_update_short_circuits_and_serializes_without_release_io().await;
}

#[tokio::test]
async fn wave11_cli_contract_update_wrappers_fail_before_remote_or_replacement_effects() {
    let mut output = Vec::new();
    let error = execute(&mut output).await.assert_error().to_string();
    assert!(
        error.contains("canonical v8 or newer release builds"),
        "{error}"
    );
    assert!(output.is_empty());

    let client = http_client().assert_value();
    let error = download(&client, "not a URL", 1, "malformed release URL")
        .await
        .assert_error()
        .to_string();
    assert!(
        error.contains("could not fetch malformed release URL"),
        "{error}"
    );

    let error = install(&test_executables(), ReleaseVersion([8, 11, 0]))
        .assert_error()
        .to_string();
    assert!(error.contains("could not verify the update"), "{error}");
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

fn test_executables() -> ReleaseExecutables {
    ReleaseExecutables {
        zeroshot: b"new binary".to_vec(),
        restic: b"new restic".to_vec(),
        restic_name: "restic",
    }
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

fn truncated_response(declared_length: usize, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {declared_length}\r\nConnection: close\r\n\r\n"
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}
