use std::path::PathBuf;

use clap::{Args, Subcommand};
use openengine_cluster_protocol::{
    FileAddress, FilePath, FileRequest, FileScope, FileSelection, IdempotencyKey, RunId,
};

use super::{ConnectionScopeArg, convert};
use crate::native_v2_cli::{FileCommand, FileCommandTransfer, NativeV2CliCommand, NativeV2CliError};

#[derive(Debug, Subcommand)]
pub(super) enum FileCliCommand {
    /// Stage a private file or folder; use the returned object as a filepath input or answer.
    Stage {
        source: PathBuf,
        #[arg(long)]
        target: Option<String>,
        /// Retry the same private upload after a lost response.
        #[arg(long)]
        operation: Option<String>,
        #[arg(long)]
        run_id: Option<String>,
    },
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
            Self::Stage {
                source,
                target,
                operation,
                run_id,
            } => (
                convert::validated_target(target)?,
                FileRequest::Stage {
                    run_id: RunId::new(run_id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string())),
                    operation: IdempotencyKey::new(
                        operation.unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
                    )
                    .map_err(|error| convert::usage(error.to_string()))?,
                },
                FileCommandTransfer::Upload(source),
            ),
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

pub(super) fn workspace_seed(value: String) -> Result<FileSelection, NativeV2CliError> {
    let (scope, value) = value
        .split_once(':')
        .ok_or_else(|| convert::usage("workspace seed must use user:PATH or org:PATH"))?;
    let scope = match scope {
        "user" => FileScope::User,
        "org" => FileScope::Org,
        _ => {
            return Err(convert::usage(
                "workspace seed must use user:PATH or org:PATH",
            ));
        }
    };
    let (selected, member) = value.split_once('#').unwrap_or((value, ""));
    Ok(FileSelection {
        address: FileAddress {
            scope,
            path: path(selected.into(), true)?,
        },
        member: path(member.into(), false)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(run: Option<&str>, target: Option<&str>) -> FileRouteArgs {
        FileRouteArgs {
            target: target.map(str::to_owned),
            scope: None,
            run: run.map(str::to_owned),
        }
    }

    #[test]
    fn malformed_file_commands_cannot_select_a_run_or_escape_a_named_path() {
        let source = PathBuf::from("source");
        let output = PathBuf::from("output");
        let invalid = vec![
            FileCliCommand::Stage {
                source: source.clone(),
                target: Some(String::new()),
                operation: None,
                run_id: None,
            },
            FileCliCommand::Stage {
                source: source.clone(),
                target: None,
                operation: Some(String::new()),
                run_id: None,
            },
            FileCliCommand::List {
                route: route(Some(""), None),
                path: None,
                member: None,
            },
            FileCliCommand::List {
                route: route(None, Some("\n")),
                path: None,
                member: None,
            },
            FileCliCommand::List {
                route: route(None, None),
                path: Some("../escape".into()),
                member: None,
            },
            FileCliCommand::List {
                route: route(None, None),
                path: Some("folder".into()),
                member: Some("../escape".into()),
            },
            FileCliCommand::Upload {
                route: route(Some("run"), None),
                source: source.clone(),
                path: "file".into(),
                operation: None,
            },
            FileCliCommand::Upload {
                route: route(None, None),
                source: source.clone(),
                path: "file".into(),
                operation: Some(String::new()),
            },
            FileCliCommand::Upload {
                route: route(None, None),
                source: source.clone(),
                path: String::new(),
                operation: None,
            },
            FileCliCommand::Download {
                route: route(Some("run"), None),
                path: "../escape".into(),
                output: output.clone(),
                member: String::new(),
            },
            FileCliCommand::Download {
                route: route(None, None),
                path: "file".into(),
                output,
                member: "../escape".into(),
            },
            FileCliCommand::Remove {
                route: route(Some("run"), None),
                path: "file".into(),
            },
            FileCliCommand::Remove {
                route: route(None, None),
                path: String::new(),
            },
        ];
        for command in invalid {
            assert!(command.into_command().is_err());
        }
    }

    #[test]
    fn workspace_seed_requires_a_library_scope_and_portable_member() {
        for selection in ["file", "run:file", "user:", "org:folder#../escape"] {
            assert!(workspace_seed(selection.into()).is_err(), "{selection}");
        }
        let selection = workspace_seed("org:folder#nested/data.csv".into()).unwrap();
        assert!(matches!(selection.address.scope, FileScope::Org));
        assert_eq!(selection.address.path.as_str(), "folder");
        assert_eq!(selection.member.as_str(), "nested/data.csv");
    }

    #[test]
    fn file_commands_keep_library_writes_and_run_publication_reads_in_their_scopes() {
        let org = || FileRouteArgs {
            target: None,
            scope: Some(ConnectionScopeArg::Org),
            run: None,
        };
        let file = |command: FileCliCommand| {
            let NativeV2CliCommand::File(file) = command.into_command().unwrap() else {
                unreachable!()
            };
            file
        };
        let listed = file(FileCliCommand::List {
            route: org(),
            path: None,
            member: None,
        });
        assert!(matches!(
            listed.request,
            FileRequest::List {
                scope: FileScope::Org,
                ..
            }
        ));
        let members = file(FileCliCommand::List {
            route: route(Some("run"), None),
            path: Some("folder".into()),
            member: Some("nested/data.csv".into()),
        });
        assert!(
            matches!(members.request, FileRequest::ListMembers { selection }
            if matches!(selection.address.scope, FileScope::Run { .. })
                && selection.member.as_str() == "nested/data.csv")
        );
        let uploaded = file(FileCliCommand::Upload {
            route: org(),
            source: PathBuf::from("source"),
            path: "folder".into(),
            operation: Some("replace".into()),
        });
        assert!(
            matches!(uploaded.request, FileRequest::Replace { address, .. }
            if matches!(address.scope, FileScope::Org))
        );
        assert_eq!(
            uploaded.transfer,
            FileCommandTransfer::Upload(PathBuf::from("source"))
        );
        let downloaded = file(FileCliCommand::Download {
            route: route(Some("run"), None),
            path: "folder".into(),
            output: PathBuf::from("output"),
            member: String::new(),
        });
        assert!(
            matches!(downloaded.request, FileRequest::Download { selection }
            if matches!(selection.address.scope, FileScope::Run { .. }))
        );
        assert_eq!(
            downloaded.transfer,
            FileCommandTransfer::Download(PathBuf::from("output"))
        );
        let removed = file(FileCliCommand::Remove {
            route: org(),
            path: "folder".into(),
        });
        assert!(matches!(removed.request, FileRequest::Remove { address }
            if matches!(address.scope, FileScope::Org)));
    }
}
