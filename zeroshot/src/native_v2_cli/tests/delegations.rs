use super::*;
use openengine_cluster_protocol::{DelegationCreateRequest, DelegationOwner};

const ORGANIZATION: &str = "00000000-0000-4000-8000-000000000001";

#[test]
fn delegation_creation_has_human_owner_and_bounded_key_expiry() {
    assert_eq!(
        parse_native_v2_args(args(&["delegation", "create", "CI"])).assert_value(),
        NativeV2CliCommand::Delegation(DelegationCommand {
            target: "cloud".to_owned(),
            action: DelegationAction::Create(DelegationCreateRequest {
                name: "CI".to_owned(),
                owner: DelegationOwner::User,
                organization_id: None,
                expires_in_days: 90,
            }),
        })
    );
    let command = parse_native_v2_args(args(&[
        "delegation",
        "create",
        "CI",
        "--owner",
        "org",
        "--organization",
        ORGANIZATION,
        "--target",
        "dev",
        "--expires-in-days",
        "365",
    ]))
    .assert_value();
    assert!(matches!(
        command,
        NativeV2CliCommand::Delegation(DelegationCommand {
            action: DelegationAction::Create(DelegationCreateRequest {
                owner: DelegationOwner::Org,
                expires_in_days: 365,
                ..
            }),
            ..
        })
    ));
}

#[test]
fn delegation_parser_rejects_unbounded_keys_and_ambiguous_owners() {
    for arguments in [
        vec!["delegation", "create", "CI", "--expires-in-days", "0"],
        vec!["delegation", "create", "CI", "--expires-in-days", "366"],
        vec!["delegation", "create", "CI", "--owner", "org"],
        vec!["delegation", "create", "CI", "--organization", ORGANIZATION],
        vec!["delegation", "show", "../secret"],
        vec!["delegation", "key", "revoke", ORGANIZATION, "bad-key-id"],
    ] {
        assert!(
            parse_native_v2_args(args(&arguments)).is_err(),
            "{arguments:?}"
        );
    }
}

#[test]
fn omitted_resource_scopes_remain_distinct_from_explicit_user_scope() {
    let command =
        parse_native_v2_args(args(&["connection", "list", "--target", "cloud"])).assert_value();
    assert!(matches!(
        command,
        NativeV2CliCommand::ConnectionList(ConnectionRoute { scope: None, .. })
    ));
    let command = parse_native_v2_args(args(&[
        "profile", "list", "--target", "cloud", "--scope", "user",
    ]))
    .assert_value();
    assert!(matches!(
        command,
        NativeV2CliCommand::ProfileList(ProfileRoute {
            scope: Some(RunProfileScope::User),
            ..
        })
    ));
}
