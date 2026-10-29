use std::collections::BTreeMap;

use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[test]
fn pi_agent_home_shorthand_uses_the_captured_user_home() {
    let invoking = Path::new("/invoking/dir");
    let home = if cfg!(windows) {
        Path::new(r"C:\Users\captured")
    } else {
        Path::new("/captured/home")
    };
    for (authored, relative) in [("~", ""), ("~/.pi/custom", ".pi/custom")] {
        let mut environment = LocalHarnessEnvironment::new(BTreeMap::from([
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            (
                "USERPROFILE".to_owned(),
                home.to_string_lossy().into_owned(),
            ),
            ("PI_CODING_AGENT_DIR".to_owned(), authored.to_owned()),
        ]));
        normalize_local(&mut environment, invoking).assert_value();
        assert_eq!(
            environment.get("PI_CODING_AGENT_DIR").map(PathBuf::from),
            Some(home.join(relative)),
            "{authored} must keep Pi's native home expansion"
        );
    }
    assert_eq!(
        agent_directory_path("~otheruser/.pi", Some(home)).assert_value(),
        PathBuf::from("~otheruser/.pi")
    );
    let backslash = agent_directory_path(r"~\.pi\custom", Some(home)).assert_value();
    assert_eq!(
        backslash,
        if cfg!(windows) {
            home.join(r".pi\custom")
        } else {
            PathBuf::from(r"~\.pi\custom")
        }
    );
}

#[test]
fn pi_agent_directory_file_urls_keep_their_native_encoding() {
    let authored = if cfg!(windows) {
        "file:///C:/Users/captured/.pi/custom%20agent%23test"
    } else {
        "file:///captured/home/.pi/custom%20agent%23test"
    };
    let mut environment = LocalHarnessEnvironment::new(BTreeMap::from([(
        "PI_CODING_AGENT_DIR".to_owned(),
        authored.to_owned(),
    )]));
    normalize_local(&mut environment, Path::new("/invoking/dir")).assert_value();
    assert_eq!(
        environment.get("PI_CODING_AGENT_DIR").map(String::as_str),
        Some(authored)
    );
}
