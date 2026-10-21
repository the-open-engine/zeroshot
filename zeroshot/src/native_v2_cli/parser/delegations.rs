use clap::{Args, Subcommand, ValueEnum};
use openengine_cluster_protocol::{
    DelegationCreateRequest, DelegationKeyCreateRequest, DelegationOwner,
};

use crate::native_v2_cli::{DelegationAction, DelegationCommand, NativeV2CliCommand, NativeV2CliError};

#[derive(Debug, Subcommand)]
#[command(
    after_long_help = "API keys act with their owner's current permissions and support every target command except delegation management. User-owned delegations follow the user's memberships; organization-owned delegations stay in one organization and require an owner or admin to manage. Keys expire after 90 days by default, with a maximum of 365 days. Creation prints the secret once. Store it in your CI secret store; do not put it in profiles, connections, or run input."
)]
pub(super) enum DelegationCliCommand {
    /// List delegation metadata without secret keys.
    List(OwnerArgs),
    /// Create a delegation and its first API key. Prints the key once.
    Create(CreateArgs),
    /// Read one delegation and its key metadata.
    Show(SelectorArgs),
    /// Disable a delegation and revoke its access.
    Disable(SelectorArgs),
    /// Create, inspect, or revoke a delegation's API keys.
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
}

#[derive(Debug, Args)]
struct RouteArgs {
    /// Configured hosted target.
    #[arg(long, default_value = "cloud", value_name = "NAME")]
    target: String,
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum OwnerArg {
    #[default]
    User,
    Org,
}

#[derive(Debug, Args)]
pub(super) struct OwnerArgs {
    #[command(flatten)]
    route: RouteArgs,
    /// Delegate your user or an organization.
    #[arg(long, value_enum, default_value = "user")]
    owner: OwnerArg,
    /// Organization UUID. Required with --owner org.
    #[arg(long, value_name = "UUID", required_if_eq("owner", "org"))]
    organization: Option<String>,
}

#[derive(Debug, Args)]
pub(super) struct CreateArgs {
    /// Display name for this machine delegation.
    name: String,
    #[command(flatten)]
    owner: OwnerArgs,
    /// API key lifetime in days (1-365).
    #[arg(long, default_value_t = 90, value_parser = clap::value_parser!(u16).range(1..=365))]
    expires_in_days: u16,
}

#[derive(Debug, Args)]
pub(super) struct SelectorArgs {
    /// Delegation UUID.
    id: String,
    #[command(flatten)]
    route: RouteArgs,
}

#[derive(Debug, Subcommand)]
pub(super) enum KeyCommand {
    /// List key metadata without secrets.
    List(SelectorArgs),
    /// Create another key for overlapping rotation. Prints the key once.
    Create(KeyCreateArgs),
    /// Revoke one key. Accepted runs continue.
    Revoke(KeyRevokeArgs),
}

#[derive(Debug, Args)]
pub(super) struct KeyCreateArgs {
    #[command(flatten)]
    selector: SelectorArgs,
    /// Optional label for this key.
    #[arg(long)]
    name: Option<String>,
    /// API key lifetime in days (1-365).
    #[arg(long, default_value_t = 90, value_parser = clap::value_parser!(u16).range(1..=365))]
    expires_in_days: u16,
}

#[derive(Debug, Args)]
pub(super) struct KeyRevokeArgs {
    #[command(flatten)]
    selector: SelectorArgs,
    /// Key UUID.
    key_id: String,
}

impl DelegationCliCommand {
    pub(super) fn into_command(self) -> Result<NativeV2CliCommand, NativeV2CliError> {
        let (target, action) = self.into_action()?;
        super::convert::validate_public_id(&target, "target name")?;
        Ok(NativeV2CliCommand::Delegation(DelegationCommand {
            target,
            action,
        }))
    }

    fn into_action(self) -> Result<(String, DelegationAction), NativeV2CliError> {
        match self {
            Self::List(args) => args.into_action(),
            Self::Create(args) => args.into_action(),
            Self::Show(args) => Ok((
                args.route.target,
                DelegationAction::Show { id: uuid(args.id)? },
            )),
            Self::Disable(args) => Ok((
                args.route.target,
                DelegationAction::Disable { id: uuid(args.id)? },
            )),
            Self::Key { command } => command.into_action(),
        }
    }
}

impl OwnerArgs {
    fn into_action(self) -> Result<(String, DelegationAction), NativeV2CliError> {
        let (owner, organization_id) = self.owner()?;
        Ok((
            self.route.target,
            DelegationAction::List {
                owner,
                organization_id,
            },
        ))
    }

    fn owner(&self) -> Result<(DelegationOwner, Option<String>), NativeV2CliError> {
        match (self.owner, self.organization.as_ref()) {
            (OwnerArg::User, None) => Ok((DelegationOwner::User, None)),
            (OwnerArg::Org, Some(id)) => Ok((DelegationOwner::Org, Some(uuid(id.clone())?))),
            _ => Err(super::convert::usage("--organization requires --owner org")),
        }
    }
}

impl CreateArgs {
    fn into_action(self) -> Result<(String, DelegationAction), NativeV2CliError> {
        let (owner, organization_id) = self.owner.owner()?;
        Ok((
            self.owner.route.target,
            DelegationAction::Create(DelegationCreateRequest {
                name: display_name(self.name)?,
                owner,
                organization_id,
                expires_in_days: self.expires_in_days,
            }),
        ))
    }
}

impl KeyCommand {
    fn into_action(self) -> Result<(String, DelegationAction), NativeV2CliError> {
        Ok(match self {
            Self::List(args) => (
                args.route.target,
                DelegationAction::KeyList { id: uuid(args.id)? },
            ),
            Self::Create(args) => (
                args.selector.route.target,
                DelegationAction::KeyCreate {
                    id: uuid(args.selector.id)?,
                    request: DelegationKeyCreateRequest {
                        name: args.name.map(display_name).transpose()?,
                        expires_in_days: args.expires_in_days,
                    },
                },
            ),
            Self::Revoke(args) => (
                args.selector.route.target,
                DelegationAction::KeyRevoke {
                    id: uuid(args.selector.id)?,
                    key_id: uuid(args.key_id)?,
                },
            ),
        })
    }
}

fn uuid(value: String) -> Result<String, NativeV2CliError> {
    uuid::Uuid::parse_str(&value)
        .map(|id| id.to_string())
        .map_err(|_| super::convert::usage("delegation, key, and organization IDs must be UUIDs"))
}

fn display_name(value: String) -> Result<String, NativeV2CliError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.chars().count() > 100 || value.chars().any(char::is_control) {
        return Err(super::convert::usage(
            "name must contain 1-100 characters without control characters",
        ));
    }
    Ok(value)
}
