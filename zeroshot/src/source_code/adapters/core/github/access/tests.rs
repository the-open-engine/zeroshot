use super::*;
use crate::source_code_provider::adapters::core::network::{ForgeNetwork, NETWORK_FIELDS};

#[test]
fn github_child_receives_only_the_ten_snapshot_network_fields() {
    let root = tempfile::tempdir().unwrap();
    let mut snapshot: BTreeMap<String, String> = NETWORK_FIELDS
        .iter()
        .map(|name| {
            (
                name.to_string(),
                format!("value-{}", name.to_ascii_uppercase()),
            )
        })
        .collect();
    snapshot.insert(
        "SSL_CERT_FILE".into(),
        root.path().join("ca.pem").to_string_lossy().into_owned(),
    );
    snapshot.insert(
        "SSL_CERT_DIR".into(),
        root.path().join("certs").to_string_lossy().into_owned(),
    );
    snapshot.insert("UNRELATED_PRIVATE_TOKEN".into(), "must-not-forward".into());
    snapshot.insert("GH_CONFIG_DIR".into(), "must-not-forward".into());
    let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig::hosted(root.path().into()))
        .with_network(ForgeNetwork::from_snapshot(&snapshot));
    let command = authority.api_command(SourceDeliveryCredential::token("test-token"));
    let environment: BTreeMap<_, _> = command
        .as_std()
        .get_envs()
        .filter_map(|(name, value)| {
            value.map(|value| {
                (
                    name.to_string_lossy().into_owned(),
                    value.to_string_lossy().into_owned(),
                )
            })
        })
        .collect();
    let effective = crate::native_v2_capsule::provider_process::LocalHarnessEnvironment::new(
        environment.clone(),
    );
    for name in NETWORK_FIELDS {
        assert_eq!(effective.get(name), Some(&snapshot[*name]));
    }
    assert!(
        !environment
            .values()
            .any(|value| value == "must-not-forward")
    );
    assert!(!format!("{authority:?}").contains("value-HTTPS_PROXY"));
}

#[cfg(unix)]
#[tokio::test]
async fn access_reads_project_and_enterprise_version_in_one_request() {
    for (base, version, name, accepted) in [
        ("https://github.com", "", "acme/project", true),
        (
            "https://ghe.example",
            "enterprise-server@3.22.2",
            "acme/project",
            true,
        ),
        ("https://ghe.example", "3.22.1", "acme/project", false),
        ("https://ghe.example", "", "acme/project", false),
        ("https://github.com", "", "other/project", false),
    ] {
        let root = tempfile::tempdir().unwrap();
        let program = root.path().join("gh");
        openengine_cluster_testkit::fixture::write_executable(&program, format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HOME/calls\"\nprintf 'HTTP/2 200 OK\\r\\nx-github-enterprise-version: {version}\\r\\n\\r\\n{{\"full_name\":\"{name}\"}}'\n"
        ), 0o700).unwrap();
        let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
            gh_program: program,
            ..GhCliAuthorityConfig::hosted(root.path().into())
        })
        .with_base_url(openengine_cluster_protocol::SourceForgeBaseUrl::new(base).unwrap())
        .unwrap();
        assert_eq!(
            authority
                .check_access(
                    "acme/project",
                    SourceDeliveryCredential::token("test-token"),
                    None,
                )
                .await
                .is_ok(),
            accepted
        );
        let calls = std::fs::read_to_string(root.path().join("calls")).unwrap();
        assert_eq!(calls.lines().count(), 1);
        assert!(calls.contains("repos/acme/project --method GET --include"));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn github_proxy_credentials_are_redacted_before_diagnostic_bounds() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("gh");
    openengine_cluster_testkit::fixture::write_executable(&program,
        "#!/bin/sh\nprintf '%s\\n' \"$HTTPS_PROXY\" >&2\nprintf 'proxy-password proxy-user\\n' >&2\nexit 1\n", 0o700).unwrap();
    let network = ForgeNetwork::from_snapshot(&BTreeMap::from([(
        "HTTPS_PROXY".into(),
        "http://proxy-user:proxy-password@127.0.0.1:1".into(),
    )]));
    let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        gh_program: program,
        ..GhCliAuthorityConfig::hosted(root.path().into())
    })
    .with_network(network);
    let error = authority
        .check_access(
            "acme/project",
            SourceDeliveryCredential::token("test-token"),
            None,
        )
        .await
        .unwrap_err();
    for secret in ["proxy-user", "proxy-password", "http://proxy-user:"] {
        assert!(!error.to_string().contains(secret), "{error}");
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn stop_request_ends_a_stalled_project_read_and_its_process_tree() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("gh");
    let recorded = root.path().join("processes");
    openengine_cluster_testkit::fixture::write_executable(
        &program,
        format!(
            "#!/bin/sh\n/bin/sleep 60 &\nprintf '%s %s\\n' \"$$\" \"$!\" > '{}'\nwait\n",
            recorded.display()
        ),
        0o700,
    )
    .unwrap();
    let authority = GhCliDeliveryAuthority::new(GhCliAuthorityConfig {
        gh_program: program,
        ..GhCliAuthorityConfig::hosted(root.path().into())
    });
    let (stop, stopping) = tokio::sync::watch::channel(false);
    let check = authority.check_access(
        "acme/project",
        SourceDeliveryCredential::token("test-token"),
        Some(stopping),
    );
    tokio::pin!(check);
    let processes = loop {
        let started = std::fs::read_to_string(&recorded).unwrap_or_default();
        let processes = started
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if processes.len() == 2 {
            break processes;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut check)
                .await
                .is_err(),
            "the stalled read ended without a stop request"
        );
    };
    stop.send(true).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(10), check)
        .await
        .expect("a stop request must end the stalled read")
        .unwrap_err();
    assert!(!error.cleanup_unconfirmed(), "{error}");
    for process in processes {
        // The capture confirmed the tree before it returned; nothing may still run.
        let running = std::fs::read_to_string(format!("/proc/{process}/stat"))
            .is_ok_and(|stat| stat.split_whitespace().nth(2) != Some("Z"));
        assert!(!running, "gh process {process} outlived the stopped read");
    }
}
