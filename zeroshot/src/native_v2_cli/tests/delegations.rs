use super::*;
use openengine_cluster_protocol::{
    DelegationCreateRequest, DelegationKeyCreateRequest, DelegationOwner,
};

const ORGANIZATION: &str = "00000000-0000-4000-8000-000000000001";

#[test]
fn delegation_creation_has_human_owner_and_bounded_key_expiry() {
    assert_eq!(
        parse_native_v2_args(args(&["delegation", "create", "CI"])).assert_value(),
        NativeV2CliCommand::Delegation(DelegationCommand {
            target: "platform".to_owned(),
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
        parse_native_v2_args(args(&["connection", "list", "--target", "platform"])).assert_value();
    assert!(matches!(
        command,
        NativeV2CliCommand::ConnectionList(ConnectionRoute { scope: None, .. })
    ));
    let command = parse_native_v2_args(args(&[
        "profile", "list", "--target", "platform", "--scope", "user",
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

#[test]
fn delegation_listing_and_owner_scope_parse_exactly() {
    assert_eq!(
        parse_native_v2_args(args(&["delegation", "list"])).assert_value(),
        NativeV2CliCommand::Delegation(DelegationCommand {
            target: "platform".to_owned(),
            action: DelegationAction::List {
                owner: DelegationOwner::User,
                organization_id: None,
            },
        })
    );

    let organization = "00000000-0000-4000-8000-0000000000AB";
    assert_eq!(
        parse_native_v2_args(args(&[
            "delegation",
            "list",
            "--owner",
            "org",
            "--organization",
            organization,
            "--target",
            "staging",
        ]))
        .assert_value(),
        NativeV2CliCommand::Delegation(DelegationCommand {
            target: "staging".to_owned(),
            action: DelegationAction::List {
                owner: DelegationOwner::Org,
                organization_id: Some(organization.to_ascii_lowercase()),
            },
        })
    );
}

#[test]
fn delegation_selectors_and_key_subcommands_parse_exactly() {
    let id = "00000000-0000-4000-8000-000000000001";
    let key_id = "00000000-0000-4000-8000-000000000002";

    assert_eq!(
        parse_native_v2_args(args(&["delegation", "show", id])).assert_value(),
        NativeV2CliCommand::Delegation(DelegationCommand {
            target: "platform".to_owned(),
            action: DelegationAction::Show { id: id.to_owned() },
        })
    );
    assert_eq!(
        parse_native_v2_args(args(&["delegation", "disable", id, "--target", "dev"]))
            .assert_value(),
        NativeV2CliCommand::Delegation(DelegationCommand {
            target: "dev".to_owned(),
            action: DelegationAction::Disable { id: id.to_owned() },
        })
    );
    assert_eq!(
        parse_native_v2_args(args(&["delegation", "key", "list", id])).assert_value(),
        NativeV2CliCommand::Delegation(DelegationCommand {
            target: "platform".to_owned(),
            action: DelegationAction::KeyList { id: id.to_owned() },
        })
    );
    assert_eq!(
        parse_native_v2_args(args(&[
            "delegation",
            "key",
            "create",
            id,
            "--name",
            "  rotation  ",
            "--expires-in-days",
            "1",
            "--target",
            "dev",
        ]))
        .assert_value(),
        NativeV2CliCommand::Delegation(DelegationCommand {
            target: "dev".to_owned(),
            action: DelegationAction::KeyCreate {
                id: id.to_owned(),
                request: DelegationKeyCreateRequest {
                    name: Some("rotation".to_owned()),
                    expires_in_days: 1,
                },
            },
        })
    );
    assert_eq!(
        parse_native_v2_args(args(&["delegation", "key", "create", id])).assert_value(),
        NativeV2CliCommand::Delegation(DelegationCommand {
            target: "platform".to_owned(),
            action: DelegationAction::KeyCreate {
                id: id.to_owned(),
                request: DelegationKeyCreateRequest {
                    name: None,
                    expires_in_days: 90,
                },
            },
        })
    );
    assert_eq!(
        parse_native_v2_args(args(&["delegation", "key", "revoke", id, key_id])).assert_value(),
        NativeV2CliCommand::Delegation(DelegationCommand {
            target: "platform".to_owned(),
            action: DelegationAction::KeyRevoke {
                id: id.to_owned(),
                key_id: key_id.to_owned(),
            },
        })
    );
}

#[test]
fn delegation_parser_rejects_unbounded_names_identifiers_and_targets() {
    let id = "00000000-0000-4000-8000-000000000001";
    let too_long_name = "n".repeat(101);
    let too_long_target = "t".repeat(257);
    for arguments in [
        vec!["delegation", "create", ""],
        vec!["delegation", "create", "   "],
        vec!["delegation", "create", too_long_name.as_str()],
        vec!["delegation", "create", "line\nbreak"],
        vec![
            "delegation",
            "create",
            "CI",
            "--target",
            too_long_target.as_str(),
        ],
        vec![
            "delegation",
            "create",
            "CI",
            "--owner",
            "org",
            "--organization",
            "not-a-uuid",
        ],
        vec!["delegation", "show", id, "--target", ""],
        vec!["delegation", "disable", "not-a-uuid"],
        vec!["delegation", "key", "list", "not-a-uuid"],
        vec![
            "delegation",
            "key",
            "create",
            id,
            "--expires-in-days",
            "366",
        ],
        vec![
            "delegation",
            "key",
            "create",
            id,
            "--name",
            too_long_name.as_str(),
        ],
        vec!["delegation", "key", "revoke", id, "not-a-uuid"],
    ] {
        assert!(
            parse_native_v2_args(args(&arguments)).is_err(),
            "{arguments:?}"
        );
    }
}
