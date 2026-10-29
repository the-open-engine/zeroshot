use super::{
    environment_names_match_for_platform, private_command_environment_name_for_platform,
    secret_environment_name_for_platform,
};

#[test]
fn windows_secret_environment_names_are_case_insensitive_and_canonical() {
    assert!(environment_names_match_for_platform("Path", "PATH", true));
    assert!(!environment_names_match_for_platform("Path", "PATH", false));
    assert_eq!(
        secret_environment_name_for_platform("Helper_Secret", true),
        "HELPER_SECRET"
    );
    assert!(!private_command_environment_name_for_platform(
        "Gh_Token", true
    ));
    assert!(!private_command_environment_name_for_platform(
        "Copilot_Provider_Api_Key",
        true
    ));
}
