use std::io::{Seek, Write};

use openengine_cluster_protocol::{FileRequest, FileResponse};
use zeroshot_engine::native_v2_cli::{FileCommand, FileCommandTransfer, NativeV2CliError};
use zeroshot_engine::native_v2_files::{
    FileAccess, FileReplaceRequest, FileTransfer, TargetFileHost, new_transfer, pack, unpack,
};

use super::{
    FileTargetRegistry, TargetControlAuthority, TargetHttpControlAuthority, TargetRegistry,
    default_target_registry_path,
};
use super::controller_authority::files::FileReply;

pub async fn execute(
    command: FileCommand,
    output: &mut impl Write,
) -> Result<(), NativeV2CliError> {
    // Validate the receipt before touching a target or publishing into a caller-owned path.
    let download_receipt = match &command.transfer {
        FileCommandTransfer::Download(destination) => Some(serde_json::to_value(destination)?),
        _ => None,
    };
    // Only the explicitly selected local store is opened. Named targets never fall back locally.
    let local = if command.target.is_none() {
        Some(zeroshot_engine::native_v2_cli::files::local_file_store()?)
    } else {
        None
    };
    let scratch = match &local {
        Some((_, _, transfers)) => transfers.clone(),
        None => {
            // Scratch holds transport bytes only and is never a scope resolver.
            let path = default_target_registry_path().map_err(|error| error.to_cli_error())?;
            path.parent()
                .ok_or_else(|| {
                    NativeV2CliError::Target("file transfer directory is unavailable".to_owned())
                })?
                .join("file-transfers")
        }
    };
    zeroshot_engine::native_v2_files::reclaim_transfers(&scratch)?;
    let mut staged = new_transfer(&scratch)?;
    if let FileCommandTransfer::Upload(source) = &command.transfer {
        pack(source, &mut staged)?;
        staged.as_file().sync_all()?;
        staged.rewind()?;
    }
    let reply = match (&command.target, local) {
        (Some(name), _) => {
            let registry = FileTargetRegistry::new(
                default_target_registry_path().map_err(|error| error.to_cli_error())?,
            );
            let target = registry.get(name).map_err(|error| error.to_cli_error())?;
            let authority = TargetHttpControlAuthority::production()
                .map_err(|error| error.into_cli(&target))?;
            authority
                .file_request(&target, &command.request, (&staged, &scratch))
                .await
                .map_err(|error| authority.redact_error(error).into_cli(&target))?
        }
        (None, Some((store, identity, _))) => {
            let access = FileAccess {
                target: "local",
                identity: &identity,
            };
            local_file_request(&store, &access, command.request, staged)?
        }
        _ => {
            return Err(NativeV2CliError::Target(
                "selected file target is unavailable".to_owned(),
            ));
        }
    };
    write_reply(reply, command.transfer, download_receipt, output)
}

fn local_file_request(
    store: &impl TargetFileHost,
    access: &FileAccess<'_>,
    request: FileRequest,
    mut staged: FileTransfer,
) -> Result<FileReply, NativeV2CliError> {
    Ok(match request {
        FileRequest::List { scope, path } => FileReply::Json(FileResponse::List {
            paths: store.list(access, scope, &path)?,
        }),
        FileRequest::ListMembers { selection } => FileReply::Json(FileResponse::Members {
            entries: store.list_members(access, &selection)?,
            selection,
        }),
        FileRequest::Replace { address, operation } => {
            store.replace(
                access,
                &FileReplaceRequest {
                    address: address.clone(),
                    operation: operation.clone(),
                },
                &mut staged,
            )?;
            FileReply::Json(FileResponse::Replaced { address, operation })
        }
        FileRequest::Remove { address } => FileReply::Json(FileResponse::Removed {
            removed: store.remove(access, &address)?,
        }),
        FileRequest::Download { selection } => {
            store.download(access, &selection, &mut staged)?;
            staged.rewind()?;
            FileReply::Tree(staged)
        }
    })
}

fn write_reply(
    reply: FileReply,
    transfer: FileCommandTransfer,
    download_receipt: Option<serde_json::Value>,
    output: &mut impl Write,
) -> Result<(), NativeV2CliError> {
    match reply {
        FileReply::Json(response) => serde_json::to_writer(&mut *output, &response)?,
        FileReply::Tree(mut staged) => {
            let FileCommandTransfer::Download(destination) = transfer else {
                return Err(NativeV2CliError::Target(
                    "unexpected file tree response".to_owned(),
                ));
            };
            unpack(staged.as_file_mut(), &destination)?;
            serde_json::to_writer(
                &mut *output,
                &serde_json::json!({"output": download_receipt}),
            )?;
        }
    }
    writeln!(output)?;
    Ok(())
}
