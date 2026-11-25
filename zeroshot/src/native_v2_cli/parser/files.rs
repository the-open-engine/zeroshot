use std::path::PathBuf;

use clap::{Args, Subcommand};
use openengine_cluster_protocol::{
    FileAddress, FilePath, FileRequest, FileScope, FileSelection, IdempotencyKey, RunId,
};

use super::{ConnectionScopeArg, convert};
use crate::native_v2_cli::{FileCommand, FileCommandTransfer, NativeV2CliCommand, NativeV2CliError};

#[derive(Debug, Subcommand)]
pub(super) enum FileCliCommand {
    /// List named paths, or list the members of one file/tree when PATH is supplied.
    List {
        #[command(flatten)]
        route: FileRouteArgs,
        path: Option<String>,
        /// Directory or file within the selected tree. Requires PATH.
        #[arg(long, requires = "path")]
        member: Option<String>,
    },
    /// Upload a file or directory and atomically replace the complete named path.
    Upload {
        #[command(flatten)]
        route: FileRouteArgs,
        source: PathBuf,
        path: String,
        /// Stable retry identity. Reusing it with different bytes fails.
        #[arg(long)]
        operation: Option<String>,
    },
    /// Download a file or complete directory to a new local destination.
    Download {
        #[command(flatten)]
        route: FileRouteArgs,
        path: String,
        output: PathBuf,
        /// Select one member or subtree; defaults to the complete named path.
        #[arg(long, default_value = "")]
        member: String,
    },
    /// Remove one whole reusable path. Run publications are read-only.
    Remove {
        #[command(flatten)]
        route: FileRouteArgs,
        path: String,
    },
}

#[derive(Debug, Args)]
pub(super) struct FileRouteArgs {
    /// Select this target's store. Omission selects local target state.
    #[arg(long)]
    target: Option<String>,
    /// User or organization library scope.
    #[arg(long, value_enum, conflicts_with = "run")]
    scope: Option<ConnectionScopeArg>,
    /// Read host-managed publications for this run.
    #[arg(long, value_name = "RUN_ID")]
    run: Option<String>,
}

impl FileRouteArgs {
    fn resolve(self) -> Result<(Option<String>, FileScope), NativeV2CliError> {
        let scope = match self.run {
            Some(run_id)
                if !run_id.is_empty()
                    && run_id.len() <= 256
                    && !run_id.chars().any(char::is_control) =>
            {
                FileScope::Run {
                    run_id: RunId::new(run_id),
                }
            }
            Some(_) => return Err(convert::usage("invalid run ID")),
            None => match self.scope {
                Some(ConnectionScopeArg::Org) => FileScope::Org,
                _ => FileScope::User,
            },
        };
        Ok((convert::validated_target(self.target)?, scope))
    }
}

fn path(value: String, named: bool) -> Result<FilePath, NativeV2CliError> {
    let value = FilePath::new(value).map_err(convert::usage)?;
    if named && value.as_str().is_empty() {
        return Err(convert::usage("file path must not be empty"));
    }
    Ok(value)
}

impl FileCliCommand {
    pub(super) fn into_command(self) -> Result<NativeV2CliCommand, NativeV2CliError> {
        let (target, request, transfer) = match self {
            Self::List {
                route,
                path: selected,
                member,
            } => {
                let (target, scope) = route.resolve()?;
                let request = match selected {
                    Some(selected) => FileRequest::ListMembers {
                        selection: FileSelection {
                            address: FileAddress {
                                scope,
                                path: path(selected, true)?,
                            },
                            member: path(member.unwrap_or_default(), false)?,
                        },
                    },
                    None => FileRequest::List {
                        scope,
                        path: path(String::new(), false)?,
                    },
                };
                (target, request, FileCommandTransfer::None)
            }
            Self::Upload {
                route,
                source,
                path: selected,
                operation,
            } => {
                let (target, scope) = route.resolve()?;
                writable(&scope)?;
                let operation = IdempotencyKey::new(
                    operation.unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
                )
                .map_err(|error| convert::usage(error.to_string()))?;
                (
                    target,
                    FileRequest::Replace {
                        address: FileAddress {
                            scope,
                            path: path(selected, true)?,
                        },
                        operation,
                    },
                    FileCommandTransfer::Upload(source),
                )
            }
            Self::Download {
                route,
                path: selected,
                output,
                member,
            } => {
                let (target, scope) = route.resolve()?;
                (
                    target,
                    FileRequest::Download {
                        selection: FileSelection {
                            address: FileAddress {
                                scope,
                                path: path(selected, true)?,
                            },
                            member: path(member, false)?,
                        },
                    },
                    FileCommandTransfer::Download(output),
                )
            }
            Self::Remove {
                route,
                path: selected,
            } => {
                let (target, scope) = route.resolve()?;
                writable(&scope)?;
                (
                    target,
                    FileRequest::Remove {
                        address: FileAddress {
                            scope,
                            path: path(selected, true)?,
                        },
                    },
                    FileCommandTransfer::None,
                )
            }
        };
        Ok(NativeV2CliCommand::File(FileCommand {
            target,
            request,
            transfer,
        }))
    }
}

fn writable(scope: &FileScope) -> Result<(), NativeV2CliError> {
    if matches!(scope, FileScope::Run { .. }) {
        return Err(convert::usage(
            "run publications are host-written and read-only",
        ));
    }
    Ok(())
}
