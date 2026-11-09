use std::cell::Cell;
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::thread::JoinHandle;

use flate2::{Compression, write::GzEncoder};
use openengine_cluster_testkit::assertions::{AssertError, AssertValue};

use super::*;

mod recovery_tests;

fn rerun_without_skill_opt_out(test_name: &str) -> bool {
    if std::env::var_os("ZEROSHOT_SKIP_SKILLS").is_none() {
        return false;
    }
    // Unset the opt-out only in a child, preserving concurrent tests' environment.
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test_name, "--nocapture"])
        .env_remove("ZEROSHOT_SKIP_SKILLS")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
    true
}

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

const PROXY_FIXTURE: &str = "ZEROSHOT_UPDATE_PROXY_FIXTURE";
const PROXIED_LATEST_RELEASE_URL: &str =
    "https://zeroshot-update.invalid/repos/the-open-engine/zeroshot/releases/latest";
const SIGNED_QUERY: &str = "X-Amz-Signature=zeroshot-update-test-signature";
// Bounds this test only. The updater's idle timeout must report a silent connection well
// before this generous limit, so a slow but live link keeps a tolerant idle allowance.
const STALL_DETECTION_LIMIT: Duration = Duration::from_secs(120);

#[tokio::test]
async fn update_failures_name_their_cause() {
    if let Some(report) = std::env::var_os(PROXY_FIXTURE) {
        // Child process: like `zeroshot update`, the client takes its proxy from HTTPS_PROXY.
        let client = http_client().assert_value();
        let error = latest_version_at(&client, PROXIED_LATEST_RELEASE_URL)
            .await
            .assert_error()
            .to_string();
        fs::write(report, error).unwrap();
        return;
    }

    let closed_proxy = closed_local_address();
    let proxy_down = latest_release_error_through_proxy(&format!("http://{closed_proxy}"));

    let (refusing_proxy, connect_request) = refusing_connect_proxy();
    let proxy_refused = latest_release_error_through_proxy(&refusing_proxy);
    let connect_request = connect_request
        .recv_timeout(Duration::from_secs(30))
        .expect("the update request did not reach HTTPS_PROXY");
    assert!(
        connect_request.starts_with(b"CONNECT zeroshot-update.invalid:443 "),
        "{}",
        String::from_utf8_lossy(&connect_request)
    );

    let github = TestServer::start(vec![github_rate_limit_response()]);
    let client = http_client().assert_value();
    let rate_limited = latest_version_at(
        &client,
        &github.url(&format!(
            "repos/the-open-engine/zeroshot/releases/latest?{SIGNED_QUERY}"
        )),
    )
    .await
    .assert_error()
    .to_string();
    github.finish();

    let messages = [&proxy_down, &proxy_refused, &rate_limited];
    assert!(
        names_refused_connection(&proxy_down, closed_proxy),
        "an unreachable proxy is not named as a refused connection: {messages:#?}"
    );
    let refused = proxy_refused.to_lowercase();
    assert!(
        refused.contains("tunnel") || refused.contains("proxy"),
        "a proxy that refuses CONNECT is not named: {messages:#?}"
    );
    assert!(
        rate_limited.contains("403") && rate_limited.contains("API rate limit exceeded"),
        "GitHub's rate-limit explanation is missing: {messages:#?}"
    );
    assert!(
        !rate_limited.contains("zeroshot-update-test-signature"),
        "{rate_limited}"
    );
    assert!(
        proxy_down != proxy_refused && proxy_refused != rate_limited && proxy_down != rate_limited,
        "different failures print the same message: {messages:#?}"
    );
}

#[tokio::test]
async fn update_failures_report_stalls_as_timeouts_without_cutting_off_progressing_downloads() {
    let silent = SilentServer::start(b"");
    let stalled = SilentServer::start(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100000\r\n\r\n{\"tag_",
    );
    let chunk = b"0123456789abcdef".repeat(512);
    let progressing = ProgressingServer::start(chunk.clone(), Duration::from_millis(500));
    let client = http_client().assert_value();
    let silent_url = silent.url(&format!("releases/latest?{SIGNED_QUERY}"));
    let stalled_url = stalled.url(&format!("releases/latest?{SIGNED_QUERY}"));
    let archive_url = progressing.url("zeroshot-release.tar.gz");

    let outcome = tokio::time::timeout(STALL_DETECTION_LIMIT, async {
        tokio::join!(
            download(&client, &archive_url, MAX_ARCHIVE_BYTES, "release archive"),
            async {
                // Start after the progressing download, so it is always the older request.
                tokio::time::sleep(Duration::from_secs(1)).await;
                let stalls = tokio::join!(
                    latest_version_at(&client, &silent_url),
                    latest_version_at(&client, &stalled_url),
                );
                progressing.finish();
                stalls
            }
        )
    })
    .await;
    let Ok((archive, (silent_result, stalled_result))) = outcome else {
        panic!(
            "a server that stopped sending kept the update waiting for {STALL_DETECTION_LIMIT:?}"
        );
    };

    for (case, result) in [
        ("sending no response", silent_result),
        ("stopping mid-body", stalled_result),
    ] {
        let error = result.assert_error_with(case).to_string();
        let wording = error.to_lowercase();
        assert!(
            wording.contains("timed out") || wording.contains("timeout"),
            "a server {case} is not reported as a timeout: {error}"
        );
        assert!(!error.contains("zeroshot-update-test-signature"), "{error}");
    }
    let archive = archive
        .unwrap_or_else(|error| panic!("a download that kept receiving data was cut off: {error}"));
    let chunks = progressing.join();
    assert!(
        chunks > 1,
        "the progressing download received {chunks} chunk(s)"
    );
    assert_eq!(archive, chunk.repeat(chunks));
}

#[tokio::test]
async fn github_failure_explanations_are_bounded_single_line_text() {
    // The updater's limits for a failure body and for the message shown from it.
    const BODY_LIMIT: usize = 64 * 1024;
    const MESSAGE_LIMIT: usize = 512;
    let message = format!(
        "\u{1b}[2Jrate\r\nlimited\t{}{}",
        "é".repeat(MESSAGE_LIMIT),
        "z".repeat(MESSAGE_LIMIT)
    );
    let explained = serde_json::json!({ "message": message }).to_string();
    let oversized =
        serde_json::json!({ "message": "hidden", "padding": "x".repeat(BODY_LIMIT) }).to_string();
    let declared = format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{{\"message\":\"hidden\"}}",
        BODY_LIMIT + 1
    );
    let chunked = format!(
        "HTTP/1.1 403 Forbidden\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{oversized}\r\n0\r\n\r\n",
        oversized.len()
    );
    let server = TestServer::start(vec![
        fixed_response("403 Forbidden", explained.as_bytes()),
        declared.into_bytes(),
        chunked.into_bytes(),
    ]);
    let client = http_client().assert_value();
    let mut errors = Vec::new();
    for label in ["explained", "declared", "chunked"] {
        let error = download(&client, &server.url(label), 64, label).await;
        errors.push(error.assert_error().to_string());
    }
    server.finish();

    let explanation = errors[0]
        .strip_prefix("update failed: could not fetch explained: HTTP 403 Forbidden: ")
        .unwrap_or_else(|| panic!("GitHub's explanation is missing: {}", errors[0]));
    assert!(explanation.len() <= MESSAGE_LIMIT, "{explanation}");
    assert!(
        !explanation.chars().any(char::is_control),
        "{explanation:?}"
    );
    assert!(
        explanation.starts_with("[2Jrate") && explanation.contains("limited é"),
        "{explanation}"
    );
    assert!(explanation.ends_with('z'), "{explanation}");
    for (label, error) in [("declared", &errors[1]), ("chunked", &errors[2])] {
        assert_eq!(
            error,
            &format!("update failed: could not fetch {label}: HTTP 403 Forbidden")
        );
    }
}

#[tokio::test]
async fn wave5_cli_contract_release_metadata_and_verified_assets_are_self_consistent() {
    if rerun_without_skill_opt_out(
        "native_v2_cli::update::tests::wave5_cli_contract_release_metadata_and_verified_assets_are_self_consistent",
    ) {
        return;
    }
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
    let memory_skill = b"---\nname: zeroshot-memory-init\ndescription: Memory initialization\n---\n\nInitialize.\n";
    let graph_skill =
        b"---\nname: zeroshot-graph-design\ndescription: Graph design\n---\n\nDesign.\n";
    let plan_skill = b"---\nname: zeroshot-plan\ndescription: Planning\n---\n\nPlan.\n";
    let manifest = format!(
        "{:x}  {SKILL_ASSET}\n{:x}  zeroshot-memory-init-skill.md\n{:x}  zeroshot-graph-design-skill.md\n{:x}  zeroshot-plan-skill.md\n{:x}  {filename}\n",
        Sha256::digest(canonical_skill),
        Sha256::digest(memory_skill),
        Sha256::digest(graph_skill),
        Sha256::digest(plan_skill),
        Sha256::digest(&archive),
    );
    let assets = TestServer::start(vec![
        fixed_response("200 OK", manifest.as_bytes()),
        fixed_response("200 OK", canonical_skill),
        fixed_response("200 OK", memory_skill),
        fixed_response("200 OK", graph_skill),
        fixed_response("200 OK", plan_skill),
        fixed_response("200 OK", &archive),
    ]);
    let release = download_release_at(&client, ReleaseVersion([8, 2, 0]), latest, &assets.base_url)
        .await
        .assert_value();
    assert_eq!(release.skills.len(), SKILL_ASSETS.len());
    let executables = release.executables.assert_value();
    assert_eq!(executables.zeroshot, binary);
    assert_eq!(executables.restic, restic);
    assert_eq!(executables.restic_name, restic_name);
    assets.finish();

    let unchanged = TestServer::start(vec![
        fixed_response("200 OK", manifest.as_bytes()),
        fixed_response("200 OK", canonical_skill),
        fixed_response("200 OK", memory_skill),
        fixed_response("200 OK", graph_skill),
        fixed_response("200 OK", plan_skill),
    ]);
    let same_version = download_release_at(&client, latest, latest, &unchanged.base_url)
        .await
        .assert_value();
    assert!(same_version.executables.is_none());
    assert_eq!(same_version.skills.len(), SKILL_ASSETS.len());
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
        let write_executable = |path: &Path, contents: &[u8]| {
            openengine_cluster_testkit::fixture::write_executable(path, contents, 0o755)
                .assert_value()
        };
        let directory = tempfile::tempdir().assert_value();
        let candidate = directory.path().join("zeroshot");
        write_executable(&candidate, b"#!/bin/sh\nprintf 'zeroshot 8.2.1\\n'\n");
        smoke(&candidate, version).assert_value();
        assert!(smoke(&candidate, ReleaseVersion([8, 2, 2])).is_err());
        let missing = directory.path().join("missing");
        assert!(make_executable(&missing).is_err());
        assert!(smoke(&missing, version).is_err());

        let restic = directory.path().join("restic");
        write_executable(
            &restic,
            b"#!/bin/sh\nprintf 'restic 0.19.1 compiled with go1.25\\n'\n",
        );
        smoke_restic(&restic).assert_value();
        for invalid in [
            b"#!/bin/sh\nprintf 'restic 0.19.0 compiled with go1.25\\n'\n".as_slice(),
            b"#!/bin/sh\nexit 1\n".as_slice(),
        ] {
            write_executable(&restic, invalid);
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
                read_request_head(&mut stream);
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

fn read_request_head(stream: &mut TcpStream) -> Vec<u8> {
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
    request
}

fn local_listener() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    (listener, base_url)
}

fn closed_local_address() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// Whether `message` names the refused connection to `address` in this OS's words.
fn names_refused_connection(message: &str, address: SocketAddr) -> bool {
    let refused = TcpStream::connect(address).assert_error();
    assert_eq!(
        refused.kind(),
        std::io::ErrorKind::ConnectionRefused,
        "{refused}"
    );
    let os_words = refused.to_string();
    let os_words = os_words
        .split(" (os error")
        .next()
        .unwrap_or_default()
        .to_lowercase();
    let message = message.to_lowercase();
    message.contains(&os_words) || message.contains("connection refused")
}

/// Fetches the latest release in a child whose only proxy setting is `HTTPS_PROXY`.
fn latest_release_error_through_proxy(proxy: &str) -> String {
    let report = tempfile::tempdir().unwrap();
    let path = report.path().join("error.txt");
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        "native_v2_cli::update::tests::update_failures_name_their_cause",
        "--nocapture",
    ]);
    for name in [
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        command.env_remove(name);
    }
    let output = command
        .env("HTTPS_PROXY", proxy)
        .env(PROXY_FIXTURE, &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read_to_string(path).unwrap()
}

/// A proxy that answers one CONNECT with 403 Forbidden and reports the request it got.
fn refusing_connect_proxy() -> (String, mpsc::Receiver<Vec<u8>>) {
    let (listener, url) = local_listener();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let request = read_request_head(&mut stream);
        stream
            .write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        stream.flush().unwrap();
        let _ = sender.send(request);
    });
    (url, receiver)
}

/// GitHub's REST API answer once an unauthenticated client exhausts its rate limit.
fn github_rate_limit_response() -> Vec<u8> {
    let body = br##"{"message":"API rate limit exceeded for 203.0.113.7. (But here's the good news: Authenticated requests get a higher rate limit. Check out the documentation for more details.)","documentation_url":"https://docs.github.com/rest/overview/resources-in-the-rest-api#rate-limiting"}"##;
    let mut response = format!(
        "HTTP/1.1 403 rate limit exceeded\r\nContent-Type: application/json; charset=utf-8\r\nX-RateLimit-Limit: 60\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset: 1762354301\r\nX-RateLimit-Resource: core\r\nX-RateLimit-Used: 60\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

/// Answers one request with `head` (possibly nothing), then keeps the connection open
/// without sending anything more until dropped: a server that stopped responding.
struct SilentServer {
    base_url: String,
    _release: mpsc::Sender<()>,
}

impl SilentServer {
    fn start(head: &'static [u8]) -> Self {
        let (listener, base_url) = local_listener();
        let (release, released) = mpsc::channel::<()>();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request_head(&mut stream);
            stream.write_all(head).unwrap();
            stream.flush().unwrap();
            let _ = released.recv();
        });
        Self {
            base_url,
            _release: release,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base_url)
    }
}

/// Streams a chunked body, one `chunk` per `interval`, until `finish` is called.
struct ProgressingServer {
    base_url: String,
    finish: mpsc::Sender<()>,
    thread: JoinHandle<usize>,
}

impl ProgressingServer {
    fn start(chunk: Vec<u8>, interval: Duration) -> Self {
        let (listener, base_url) = local_listener();
        let (finish, finished) = mpsc::channel::<()>();
        let thread = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request_head(&mut stream);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
            let mut frame = format!("{:x}\r\n", chunk.len()).into_bytes();
            frame.extend_from_slice(&chunk);
            frame.extend_from_slice(b"\r\n");
            let mut sent = 0;
            loop {
                if stream
                    .write_all(&frame)
                    .and_then(|()| stream.flush())
                    .is_err()
                {
                    return sent;
                }
                sent += 1;
                if !matches!(
                    finished.recv_timeout(interval),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    break;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n");
            sent
        });
        Self {
            base_url,
            finish,
            thread,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{path}", self.base_url)
    }

    fn finish(&self) {
        let _ = self.finish.send(());
    }

    fn join(self) -> usize {
        self.thread.join().unwrap()
    }
}

#[tokio::test]
async fn skill_opt_out_updates_executables_without_touching_user_skills() {
    const FIXTURE: &str = "ZEROSHOT_SKILL_OPT_OUT_FIXTURE";
    let Some(home) = std::env::var_os(FIXTURE).map(std::path::PathBuf::from) else {
        // Keep the opt-out and home overrides isolated from concurrent tests.
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        fs::create_dir_all(home.join(".agents/skills/zeroshot")).unwrap();
        fs::write(
            home.join(".agents/skills/zeroshot/SKILL.md"),
            b"user edit\n",
        )
        .unwrap();
        fs::write(home.join(".claude"), b"blocked destination\n").unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "native_v2_cli::update::tests::skill_opt_out_updates_executables_without_touching_user_skills",
                "--nocapture",
            ])
            .env(FIXTURE, &home)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
            .env("ZEROSHOT_SKIP_SKILLS", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        return;
    };

    let latest = ReleaseVersion([8, 2, 1]);
    let (target, executable, restic) = release_target().unwrap();
    let filename = format!("zeroshot-v{latest}-{target}.tar.gz");
    let archive = test_archive(&[
        ArchiveEntry::File(executable, b"new binary"),
        ArchiveEntry::File(restic, b"new restic"),
    ]);
    // No skill assets or checksums are available: opting out needs only the executables.
    let manifest = format!("{:x}  {filename}\n", Sha256::digest(&archive));
    let client = http_client().unwrap();
    let native = tempfile::tempdir().unwrap();
    let candidate = native.path().join(executable);
    fs::write(&candidate, b"old binary").unwrap();
    for current in [ReleaseVersion([8, 2, 0]), latest] {
        let mut responses = vec![fixed_response("200 OK", manifest.as_bytes())];
        if current < latest {
            responses.push(fixed_response("200 OK", &archive));
        }
        let server = TestServer::start(responses);
        let release = download_release_at(&client, current, latest, &server.base_url)
            .await
            .unwrap();
        server.finish();
        assert!(release.skills.is_empty());
        assert!(!install_skills(&release.skills).unwrap());
        assert_eq!(release.executables.is_some(), current < latest);
        if let Some(executables) = release.executables {
            install_at(
                &candidate,
                &executables,
                latest,
                InstallOperations {
                    verify: |_: &Path, _: ReleaseVersion| Ok(()),
                    verify_restic: |_: &Path| Ok(()),
                    replace: |staged: &Path| {
                        fs::copy(staged, &candidate)?;
                        Ok(())
                    },
                },
            )
            .unwrap();
        }
        assert_eq!(fs::read(&candidate).unwrap(), b"new binary");
        assert_eq!(fs::read(native.path().join(restic)).unwrap(), b"new restic");
        assert_eq!(
            fs::read(home.join(".agents/skills/zeroshot/SKILL.md")).unwrap(),
            b"user edit\n"
        );
        assert_eq!(
            fs::read(home.join(".claude")).unwrap(),
            b"blocked destination\n"
        );
        assert_eq!(
            fs::read_dir(home.join(".agents/skills")).unwrap().count(),
            1
        );
    }
}

#[tokio::test]
async fn a_verified_but_invalid_auxiliary_skill_aborts_release_preparation() {
    if rerun_without_skill_opt_out(
        "native_v2_cli::update::tests::a_verified_but_invalid_auxiliary_skill_aborts_release_preparation",
    ) {
        return;
    }
    let canonical = b"---\nname: zeroshot\n---\n";
    let memory = b"---\nname: zeroshot-memory-init\n---\n";
    let graph = b"---\nname: zeroshot-graph-design\n---\n";
    let invalid = [0xff];
    for invalid_index in [1, 2] {
        let mut documents: [&[u8]; 3] = [canonical, memory, graph];
        documents[invalid_index] = &invalid;
        let manifest = SKILL_ASSETS
            .iter()
            .zip(documents)
            .map(|((_, filename), document)| {
                format!("{:x}  {filename}\n", Sha256::digest(document))
            })
            .collect::<String>();
        let mut responses = vec![fixed_response("200 OK", manifest.as_bytes())];
        responses.extend(
            documents[..=invalid_index]
                .iter()
                .map(|document| fixed_response("200 OK", document)),
        );
        let server = TestServer::start(responses);
        let client = http_client().unwrap();
        let latest = ReleaseVersion([8, 2, 1]);
        let result = download_release_at(&client, latest, latest, &server.base_url).await;
        assert!(result.err().unwrap().to_string().contains("not UTF-8"));
        server.finish();
    }
}
