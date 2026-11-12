use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use openengine_cluster_testkit::assertions::{AssertError, AssertValue};
use serde_json::json;

use super::*;
use crate::harnesses::HarnessRegistry;
use crate::native_v2_contract::{RuntimeLane, RuntimePlan};
use crate::native_v2_local::check_lane_versions;

fn plan(harness: &str, providers: &[&str]) -> RuntimePlan {
    let nodes = providers
        .iter()
        .enumerate()
        .map(|(index, provider)| {
            (
                format!("node{index}"),
                json!({
                    "kind":"agent", "model":"opaque-model",
                    "lane":{"harness":harness,"provider":provider}
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    serde_json::from_value(json!({
        "harness":harness,"provider":providers[0],"nodes":nodes
    }))
    .assert_value()
}

fn reported(harness: &str, version: &str) -> String {
    match harness {
        "codex" => format!("codex-cli {version}"),
        "claude" => format!("{version} (Claude Code)"),
        "copilot" => format!("GitHub Copilot CLI {version}."),
        "hermes" => format!("Hermes Agent v{version} (2026.9.24) stable"),
        _ => version.to_owned(),
    }
}

fn install(directory: &Path, harness: &str, output: &str, status: u8) -> PathBuf {
    #[cfg(unix)]
    {
        let path = directory.join(harness);
        let script = format!(
            "#!/bin/sh\n[ \"$*\" = --version ] || exit 91\nprintf 'probe\\n' >> probes\nprintf '%s\\n' '{}'\nexit {status}\n",
            output.replace('\'', "'\\''"),
        );
        openengine_cluster_testkit::fixture::write_executable(&path, script, 0o700).assert_value();
        path
    }
    #[cfg(windows)]
    {
        let path = directory.join(format!("{harness}.cmd"));
        let script = format!(
            "@echo off\r\nif not \"%*\"==\"--version\" exit /b 91\r\necho probe>>probes\r\necho {output}\r\nexit /b {status}\r\n"
        );
        std::fs::write(&path, script).assert_value();
        path
    }
}

fn environment(directory: &Path) -> BTreeMap<String, String> {
    BTreeMap::from([("PATH".to_owned(), directory.to_string_lossy().into_owned())])
}

#[tokio::test]
async fn every_declared_minimum_admits_the_boundary_and_refuses_older_or_unreadable_output() {
    let registry = HarnessRegistry::local();
    for descriptor in registry.descriptors() {
        let lane = RuntimeLane::new(descriptor.id, descriptor.providers[0]).assert_value();
        let Some(version) = registry.cli_version(&lane).assert_value() else {
            assert_eq!(descriptor.id, "opencode");
            continue;
        };
        let root = tempfile::Builder::new()
            .prefix("CLI version with spaces ")
            .tempdir()
            .assert_value();
        let minimum = display_release(version.minimum);
        let mut older = version.minimum;
        if older[2] > 0 {
            older[2] -= 1;
        } else {
            older[1] -= 1;
            older[2] = 999;
        }
        for (output, accepted) in [
            (reported(descriptor.id, &minimum), true),
            (reported(descriptor.id, &display_release(older)), false),
            (String::new(), false),
            ("unreadable native version".to_owned(), false),
        ] {
            let executable = install(root.path(), descriptor.id, &output, 0);
            let result = check_lane_versions(
                &plan(descriptor.id, &[descriptor.providers[0]]),
                &environment(root.path()),
                root.path(),
                None,
            )
            .await;
            if accepted {
                result.assert_value();
            } else {
                let detail = result.assert_error().to_string();
                for expected in [
                    lane.to_string(),
                    executable.display().to_string(),
                    minimum.clone(),
                    output,
                    version.upgrade.to_owned(),
                ] {
                    assert!(detail.contains(&expected), "missing {expected:?}: {detail}");
                }
            }
        }
    }
}

#[tokio::test]
async fn distinct_provider_lanes_probe_the_same_executable_once() {
    let root = tempfile::tempdir().assert_value();
    install(root.path(), "codex", "codex-cli 0.155.0", 0);
    check_lane_versions(
        &plan("codex", &["openai", "openrouter", "gateway"]),
        &environment(root.path()),
        root.path(),
        None,
    )
    .await
    .assert_value();
    let probes = std::fs::read_to_string(root.path().join("probes")).assert_value();
    assert_eq!(probes.lines().collect::<Vec<_>>(), ["probe"]);
}

#[tokio::test]
async fn a_valid_version_with_a_failed_command_is_refused() {
    let root = tempfile::tempdir().assert_value();
    install(root.path(), "codex", "codex-cli 0.155.0", 7);
    let detail = check_lane_versions(
        &plan("codex", &["openai"]),
        &environment(root.path()),
        root.path(),
        None,
    )
    .await
    .assert_error()
    .to_string();
    assert!(detail.contains("0.155.0"));
    assert!(detail.contains("7"));
}

#[cfg(feature = "contributed-harnesses")]
#[tokio::test]
async fn opencode_keeps_admitting_without_a_version_probe() {
    let root = tempfile::tempdir().assert_value();
    install(root.path(), "opencode", "unknown", 71);
    check_lane_versions(
        &plan("opencode", &["openai"]),
        &environment(root.path()),
        root.path(),
        None,
    )
    .await
    .assert_value();
    assert!(!root.path().join("probes").exists());
}

#[cfg(feature = "contributed-harnesses")]
#[tokio::test]
async fn hermes_refuses_the_known_incompatible_release_and_newer_versions() {
    let root = tempfile::tempdir().assert_value();
    for version in ["0.22.6", "0.22.7", "0.23.0", "0.23.1", "0.24.0", "1.0.0"] {
        install(root.path(), "hermes", &reported("hermes", version), 0);
        let detail = check_lane_versions(
            &plan("hermes", &["openai"]),
            &environment(root.path()),
            root.path(),
            None,
        )
        .await
        .assert_error()
        .to_string();
        assert!(detail.contains(">=0.22.5, <0.22.6"), "{detail}");
    }
}

#[tokio::test]
async fn probe_deadline_refuses_a_stalled_command_even_after_a_valid_version() {
    let root = tempfile::tempdir().assert_value();
    #[cfg(unix)]
    let script = "#!/bin/sh\nprintf 'codex-cli 0.155.0\\n'\nexec /bin/sleep 60\n";
    #[cfg(windows)]
    let script = "@echo off\r\necho codex-cli 0.155.0\r\n\"%SystemRoot%\\System32\\ping.exe\" -n 61 127.0.0.1 >NUL\r\n";
    #[cfg(unix)]
    let path = root.path().join("codex");
    #[cfg(windows)]
    let path = root.path().join("codex.cmd");
    #[cfg(unix)]
    openengine_cluster_testkit::fixture::write_executable(&path, script, 0o700).assert_value();
    #[cfg(windows)]
    std::fs::write(&path, script).assert_value();
    let command = ProcessSessionCommand {
        program: path.to_string_lossy().into_owned(),
        argv: vec!["--version".to_owned()],
        environment: environment(root.path()),
        workspace: WorkspaceCapability {
            current_dir: root.path().to_owned(),
            mode: WorkspaceAccessMode::ReadWrite,
        },
        deadline: Some(tokio::time::Instant::now() + Duration::from_millis(500)),
    };
    let (_, failure) = tokio::time::timeout(Duration::from_secs(20), probe(command, None))
        .await
        .assert_value();
    assert!(failure.assert_value().contains("timed out"));
}

#[test]
fn native_version_syntax_rejects_prereleases_and_wrong_harness_output() {
    let registry = HarnessRegistry::local();
    for descriptor in registry.descriptors() {
        let lane = RuntimeLane::new(descriptor.id, descriptor.providers[0]).assert_value();
        let Some(version) = registry.cli_version(&lane).assert_value() else {
            continue;
        };
        for invalid in ["", "unknown", "1.2", "1.2.3.4", "1.2.+3", "999.2.3-beta"] {
            assert!(version.accepts(&reported(descriptor.id, invalid)).is_none());
        }
    }
}

#[tokio::test]
async fn probe_uses_the_captured_native_environment_and_bounds_its_output() {
    let root = tempfile::tempdir().assert_value();
    let executable = install(root.path(), "codex", "codex-cli 0.155.0", 0);
    let source = std::fs::read_to_string(&executable).assert_value();
    #[cfg(unix)]
    let capture = "printf '%s\\n' \"$CODEX_HOME\" \"$HOME\" \"$PATH\" \"${UNDECLARED_SECRET-}\" > native-environment\n";
    #[cfg(windows)]
    let capture = "echo %CODEX_HOME%>native-environment\r\necho %HOME%>>native-environment\r\necho %PATH%>>native-environment\r\necho %UNDECLARED_SECRET%>>native-environment\r\n";
    #[cfg(unix)]
    let source = source.replacen("#!/bin/sh\n", &format!("#!/bin/sh\n{capture}"), 1);
    #[cfg(windows)]
    let source = source.replacen("@echo off\r\n", &format!("@echo off\r\n{capture}"), 1);
    #[cfg(unix)]
    openengine_cluster_testkit::fixture::write_executable(&executable, source, 0o700)
        .assert_value();
    #[cfg(windows)]
    std::fs::write(&executable, source).assert_value();
    let mut native = environment(root.path());
    let home = root
        .path()
        .join("captured-home")
        .to_string_lossy()
        .into_owned();
    native.insert("HOME".to_owned(), home.clone());
    native.insert("USERPROFILE".to_owned(), home.clone());
    native.insert("CODEX_HOME".to_owned(), home.clone());
    native.insert(
        "UNDECLARED_SECRET".to_owned(),
        "must-not-inherit".to_owned(),
    );
    check_lane_versions(&plan("codex", &["openai"]), &native, root.path(), None)
        .await
        .assert_value();
    let captured = std::fs::read_to_string(root.path().join("native-environment")).assert_value();
    let lines = captured.lines().collect::<Vec<_>>();
    assert_eq!(
        &lines[..3],
        [home.as_str(), home.as_str(), native["PATH"].as_str()]
    );
    assert!(!captured.contains("must-not-inherit"));
    install(root.path(), "codex", &"x".repeat(MAX_REPORTED_BYTES * 4), 0);
    let error = check_lane_versions(&plan("codex", &["openai"]), &native, root.path(), None)
        .await
        .assert_error()
        .to_string();
    assert!(
        error.len() < MAX_REPORTED_BYTES * 2,
        "oversized CLI diagnostic"
    );
}

#[test]
fn truncated_version_diagnostics_omit_partial_credentials_across_byte_and_chunk_boundaries() {
    let secret = "SYNTHETIC-1517-CREDENTIAL-abcdefghijklmno";
    let registry = HarnessRegistry::local();
    let lane = RuntimeLane::new("codex", "openai").assert_value();
    let range = registry.cli_version(&lane).assert_value().assert_value();
    for prefix_length in [1000, MAX_REPORTED_BYTES - 1] {
        let prefix = "codex-cli 0.155.0\n";
        let output = format!(
            "{prefix}{}{secret}",
            "x".repeat(prefix_length - prefix.len())
        );
        for chunk_size in [1, 7, MAX_REPORTED_BYTES, output.len()] {
            let mut reported = VersionOutput::default();
            for chunk in output.as_bytes().chunks(chunk_size) {
                reported.push(chunk);
            }
            assert_eq!(reported.accepted_version(range), Some([0, 155, 0]));
            let diagnostic = reported.diagnostic();
            assert!(diagnostic.contains("omitted"));
            assert!(!diagnostic.contains(&secret[..8]));
            assert!(!diagnostic.contains("xxxxxxxx"));
        }
    }
}
