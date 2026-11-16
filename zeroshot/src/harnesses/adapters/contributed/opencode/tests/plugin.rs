use std::collections::BTreeMap;

use openengine_cluster_testkit::assertions::AssertValue;

use crate::execution::WorkspaceAccessMode;
use crate::execution::driver::WorkspaceCapability;
use crate::execution::process::ProcessSessionCommand;

fn command(scratch: &std::path::Path) -> ProcessSessionCommand {
    ProcessSessionCommand {
        program: "opencode".to_owned(),
        argv: Vec::new(),
        environment: BTreeMap::from([(
            "TMPDIR".to_owned(),
            scratch.to_str().assert_value().to_owned(),
        )]),
        workspace: WorkspaceCapability {
            current_dir: scratch.to_owned(),
            mode: WorkspaceAccessMode::ReadWrite,
        },
        deadline: None,
    }
}

#[cfg(target_os = "linux")]
fn assert_owner_and_mode(path: &std::path::Path, expected_owner: (u32, u32), expected_mode: u32) {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).assert_value();
    assert_eq!((metadata.uid(), metadata.gid()), expected_owner, "{path:?}");
    assert_eq!(metadata.mode() & 0o777, expected_mode, "{path:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn root_owned_plugin_files_belong_to_the_home_owner() {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().assert_value();
    let home = root.path().join("home");
    let tmp = root.path().join("tmp");
    std::fs::create_dir(&home).assert_value();
    std::fs::create_dir(&tmp).assert_value();
    // The same test runs as root in the hosted boundary lane.
    // SAFETY: geteuid only reads the calling process's effective identity.
    if unsafe { libc::geteuid() } == 0 {
        std::os::unix::fs::chown(&home, Some(65_534), Some(65_534)).assert_value();
    }
    super::super::plugin::install(
        &mut command(&tmp),
        &home,
        "zeroshot-active-memory",
        "export default {};",
    )
    .assert_value();

    let home_metadata = std::fs::metadata(&home).assert_value();
    let expected_owner = (home_metadata.uid(), home_metadata.gid());
    let plugin_dir = home.join("zeroshot-plugins").join("zeroshot-active-memory");
    let entries = [
        (home.join("zeroshot-plugins"), 0o700),
        (plugin_dir.clone(), 0o700),
        (plugin_dir.join("package.json"), 0o600),
        (plugin_dir.join("index.js"), 0o600),
    ];
    for (path, expected_mode) in &entries {
        assert_owner_and_mode(path, expected_owner, *expected_mode);
    }

    super::super::plugin::install(
        &mut command(&tmp),
        &home,
        "zeroshot-active-memory",
        "export default {};",
    )
    .assert_value();
    for (path, expected_mode) in &entries {
        assert_owner_and_mode(path, expected_owner, *expected_mode);
    }
}

#[cfg(unix)]
#[test]
fn plugin_directories_refuse_planted_links() {
    let root = tempfile::tempdir().assert_value();
    let home = root.path().join("home");
    let tmp = root.path().join("tmp");
    let victim = root.path().join("victim");
    for directory in [&home, &tmp, &victim] {
        std::fs::create_dir(directory).assert_value();
    }
    std::os::unix::fs::symlink(&victim, home.join("zeroshot-plugins")).assert_value();
    assert!(
        super::super::plugin::install(
            &mut command(&tmp),
            &home,
            "zeroshot-active-memory",
            "export default {};",
        )
        .is_err()
    );
    assert_eq!(std::fs::read_dir(&victim).assert_value().count(), 0);
}

#[cfg(unix)]
#[test]
fn expected_marker_does_not_follow_a_planted_link() {
    let root = tempfile::tempdir().assert_value();
    let home = root.path().join("home");
    let tmp = root.path().join("tmp");
    std::fs::create_dir(&home).assert_value();
    std::fs::create_dir(&tmp).assert_value();
    super::super::plugin::install(
        &mut command(&tmp),
        &home,
        "zeroshot-active-memory",
        "export default {};",
    )
    .assert_value();

    let victim_file = root.path().join("victim_file");
    std::fs::write(&victim_file, b"untouched").assert_value();
    let marker = home
        .join("zeroshot-plugins")
        .join("expected")
        .join("zeroshot-active-memory");
    std::fs::remove_file(&marker).assert_value();
    std::os::unix::fs::symlink(&victim_file, &marker).assert_value();
    assert!(
        super::super::plugin::install(
            &mut command(&tmp),
            &home,
            "zeroshot-active-memory",
            "export default {};",
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&victim_file).assert_value(), b"untouched");
}
