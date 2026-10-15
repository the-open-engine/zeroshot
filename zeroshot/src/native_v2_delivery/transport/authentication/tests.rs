use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::time::Duration;
use openengine_cluster_testkit::assertions::AssertValue;
use crate::execution::process::{HostedProcessIdentity, HostedProcessPool, HostedProcessScope};

fn config(root: &Path) -> GitTransportConfig {
    GitTransportConfig {
        git_program: "git".into(),
        workspace: root.to_owned(),
        home_directory: root.join("caller:\nuser-home"),
        private_directory: root.to_owned(),
        identity: None,
        local_environment: None,
        deadline: Duration::from_secs(1),
    }
}

fn writer_identity() -> HostedProcessIdentity {
    // SAFETY: these calls only inspect the test's process identity.
    let (uid, gid) = unsafe { (libc::geteuid(), libc::getegid()) };
    let (uid, gid) = if uid == 0 {
        (19_771, 19_772)
    } else {
        (uid, gid)
    };
    HostedProcessPool::new(uid, gid, uid.saturating_add(1000))
        .assert_value()
        .identity(HostedProcessScope::Writer)
        .assert_value()
}

fn ssh_credential() -> SourceDeliveryCredential<'static> {
    SourceDeliveryCredential::token("").with_git(SourceGitCredential::Ssh {
        private_key: "private-test-key",
        known_hosts: "forge.invalid ssh-ed25519 test-host-key\n",
    })
}

fn environment(command: &Command) -> BTreeMap<String, String> {
    command
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
        .collect()
}

#[test]
fn contained_identity_records_are_private_bounded_and_invocation_scoped() {
    let root = tempfile::tempdir().assert_value();
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o711)).assert_value();
    let config = config(root.path());
    let identity = writer_identity();
    // This helper test exercises record/configuration ownership without loading the image library.
    let mut authentication = GitAuthentication::prepare(
        &config,
        OsStr::new("git@forge.invalid:project.git"),
        ssh_credential(),
    )
    .assert_value();
    let directory = authentication
        ._private
        .as_ref()
        .assert_value()
        .path()
        .to_owned();
    authentication
        .prepare_identity(&directory, identity)
        .assert_value();
    set_owner(&directory, Some(identity)).assert_value();
    let mut command = Command::new("git");
    command
        .env("LD_PRELOAD", "/agent-selected/library.so")
        .env("NSS_WRAPPER_PASSWD", "/agent-selected/passwd")
        .env("NSS_WRAPPER_GROUP", "/agent-selected/group");
    authentication.configure(&mut command);
    let environment = environment(&command);
    assert_eq!(environment["LD_PRELOAD"], NSS_WRAPPER_LIBRARY);
    for (variable, name) in [
        ("NSS_WRAPPER_PASSWD", "passwd"),
        ("NSS_WRAPPER_GROUP", "group"),
    ] {
        let path = PathBuf::from(&environment[variable]);
        assert_eq!(path, directory.join(name));
        let metadata = std::fs::metadata(&path).assert_value();
        assert_eq!(
            (metadata.uid(), metadata.gid()),
            (identity.uid(), identity.gid())
        );
        assert_eq!(metadata.mode() & 0o777, 0o600);
        let record = std::fs::read_to_string(path).assert_value();
        assert_eq!(record.lines().count(), 1);
        assert!(record.len() < 128);
        assert!(!record.contains("user-home"));
    }
    let passwd = std::fs::read_to_string(directory.join("passwd")).assert_value();
    let fields = passwd.trim_end().split(':').collect::<Vec<_>>();
    assert_eq!(fields.len(), 7);
    assert_eq!(fields[2], identity.uid().to_string());
    assert_eq!(fields[3], identity.gid().to_string());
    assert_eq!(fields[5], "/nonexistent");
    for (_, record) in nss_records(u32::MAX, u32::MAX) {
        assert_eq!(record.lines().count(), 1);
        assert!(record.len() < 128);
    }
    assert_eq!(
        std::fs::metadata(&directory).assert_value().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(root.path()).assert_value().mode() & 0o777,
        0o711
    );
    drop(authentication);
    assert!(
        !directory.exists(),
        "NSS records survived their Git invocation"
    );
}

#[test]
fn local_ssh_and_contained_https_do_not_select_nss_wrapper() {
    let root = tempfile::tempdir().assert_value();
    let identity = writer_identity();
    for (identity, remote, credential) in [
        (None, "git@forge.invalid:project.git", ssh_credential()),
        (
            None,
            "git@forge.invalid:project.git",
            SourceDeliveryCredential::token(""),
        ),
        (
            Some(identity),
            "https://forge.invalid/project.git",
            SourceDeliveryCredential::token("").with_git(SourceGitCredential::Https {
                username: "git-user",
                password: "private-test-password",
            }),
        ),
    ] {
        let mut config = config(root.path());
        config.identity = identity;
        let authentication =
            GitAuthentication::prepare(&config, OsStr::new(remote), credential).assert_value();
        let mut command = Command::new("git");
        authentication.configure(&mut command);
        let environment = environment(&command);
        for variable in ["LD_PRELOAD", "NSS_WRAPPER_PASSWD", "NSS_WRAPPER_GROUP"] {
            assert!(!environment.contains_key(variable), "{remote}: {variable}");
        }
    }
}

#[test]
fn contained_ssh_requires_fixed_nss_wrapper_and_cleans_preparation_failure() {
    let root = tempfile::tempdir().assert_value();
    let error = check_nss_wrapper(&root.path().join("missing-wrapper.so"))
        .expect_err("a missing dependency must fail clearly");
    assert!(
        error
            .to_string()
            .contains("contained SSH requires NSS wrapper")
    );
    let mut config = config(root.path());
    config.identity = Some(writer_identity());
    for credential in [ssh_credential(), SourceDeliveryCredential::token("")] {
        let result = GitAuthentication::prepare(
            &config,
            OsStr::new("git@forge.invalid:project.git"),
            credential,
        );
        if Path::new(NSS_WRAPPER_LIBRARY).exists() {
            let authentication = result.assert_value_with("fixed image dependency must be usable");
            let mut command = Command::new("git");
            authentication.configure(&mut command);
            let environment = environment(&command);
            assert_eq!(environment["LD_PRELOAD"], NSS_WRAPPER_LIBRARY);
            assert!(Path::new(&environment["NSS_WRAPPER_PASSWD"]).is_file());
            assert!(Path::new(&environment["NSS_WRAPPER_GROUP"]).is_file());
            drop(authentication);
        } else {
            let error = result.err().expect(
                "absent fixed wrapper must prevent SSH preparation even without a credential",
            );
            let diagnostic = error.to_string();
            assert!(diagnostic.contains("contained SSH requires NSS wrapper"));
            assert!(diagnostic.contains(NSS_WRAPPER_LIBRARY));
        }
        assert_eq!(std::fs::read_dir(root.path()).assert_value().count(), 0);
    }
}
