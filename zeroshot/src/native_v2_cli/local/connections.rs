use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::BufReader;
use crate::execution::platform::{FileAccess, private_file};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use openengine_cluster_protocol::{
    ConnectionDeleteRequest, ConnectionDeleteResult, ConnectionKey, ConnectionListRequest,
    ConnectionListResult, ConnectionMutationResult, ConnectionScope, ConnectionSetRequest,
    ConnectionSummary, EnvironmentVariableName, RunConnectionValues, RuntimePlan,
    StaticConnectionValues, STATIC_CONNECTION_KIND,
};

use super::{NativeV2CliError, local_io, local_message, prepare_private_directory};
use crate::native_v2_cli::support::{CommitPaths, cleanup_temporary, write_and_commit};
use crate::native_v2_supervisor::RunEnvironment;

const CONNECTIONS_FILE: &str = "connections.json";
const CONNECTIONS_LOCK_FILE: &str = "connections.lock";

type StoredConnections = BTreeMap<ConnectionKey, StaticConnectionValues>;

pub(super) struct LocalConnectionStore {
    root: PathBuf,
}

impl LocalConnectionStore {
    pub(super) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(super) fn list(
        &self,
        request: ConnectionListRequest,
    ) -> Result<ConnectionListResult, NativeV2CliError> {
        require_user_scope(request.scope)?;
        let lock = self.lock()?;
        FileExt::lock_shared(&lock).map_err(local_io)?;
        let connections = self.read()?;
        Ok(ConnectionListResult {
            connections: connections
                .into_iter()
                .map(|(key, values)| summary(key, values.field_names()))
                .collect(),
        })
    }

    pub(super) fn set(
        &self,
        request: ConnectionSetRequest,
    ) -> Result<ConnectionMutationResult, NativeV2CliError> {
        require_user_scope(request.scope)?;
        let lock = self.lock()?;
        lock.lock_exclusive().map_err(local_io)?;
        let mut connections = self.read()?;
        let result = ConnectionMutationResult {
            connection: summary(request.key.clone(), request.values.field_names()),
        };
        connections.insert(request.key, request.values);
        self.write(&connections)?;
        Ok(result)
    }

    pub(super) fn delete(
        &self,
        request: ConnectionDeleteRequest,
    ) -> Result<ConnectionDeleteResult, NativeV2CliError> {
        require_user_scope(request.scope)?;
        let lock = self.lock()?;
        lock.lock_exclusive().map_err(local_io)?;
        let mut connections = self.read()?;
        let deleted = connections.remove(&request.key).is_some();
        if deleted {
            self.write(&connections)?;
        }
        Ok(ConnectionDeleteResult { deleted })
    }

    pub(super) fn resolve(
        &self,
        runtime: &RuntimePlan,
        definition: Option<&openengine_cluster_protocol::RuntimeEnvironment>,
        explicit: &RunConnectionValues,
    ) -> Result<RunEnvironment, NativeV2CliError> {
        let requirements =
            openengine_cluster_protocol::run_connection_requirements(runtime, definition);
        let lock = self.lock()?;
        FileExt::lock_shared(&lock).map_err(local_io)?;
        let stored = self.read()?;
        let mut resolved = RunConnectionValues::new();
        for (key, fields) in requirements {
            resolved.insert(
                key.clone(),
                StaticConnectionValues::new(resolve_connection(&key, &fields, explicit, &stored)?)
                    .map_err(|_| local_message("resolved connection shape is invalid"))?,
            );
        }
        RunEnvironment::exact(runtime, definition, resolved).map_err(Into::into)
    }

    fn lock(&self) -> Result<File, NativeV2CliError> {
        prepare_private_directory(&self.root)?;
        let path = self.root.join(CONNECTIONS_LOCK_FILE);
        let file = private_file(&path, FileAccess::ReadWrite).map_err(local_io)?;
        Ok(file)
    }

    fn read(&self) -> Result<StoredConnections, NativeV2CliError> {
        let path = self.root.join(CONNECTIONS_FILE);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BTreeMap::new());
            }
            Err(error) => return Err(local_io(error)),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(local_message(
                "local connection store is not a regular file",
            ));
        }
        let file = private_file(&path, FileAccess::Read).map_err(local_io)?;
        serde_json::from_reader(BufReader::new(file))
            .map_err(|_| local_message("local connection store is malformed"))
    }

    fn write(&self, connections: &StoredConnections) -> Result<(), NativeV2CliError> {
        let temporary = temporary_path(&self.root);
        let mut encoded = serde_json::to_vec(connections)
            .map_err(|_| local_message("local connection store could not be encoded"))?;
        encoded.push(b'\n');
        let result = (|| {
            let file = private_file(&temporary, FileAccess::CreateNew).map_err(local_io)?;
            let destination = self.root.join(CONNECTIONS_FILE);
            write_and_commit(
                file,
                &encoded,
                CommitPaths {
                    temporary: &temporary,
                    destination: &destination,
                    parent: &self.root,
                },
            )
        })();
        cleanup_temporary(result, &temporary)
    }
}

fn resolve_connection(
    key: &ConnectionKey,
    fields: &BTreeSet<EnvironmentVariableName>,
    explicit: &RunConnectionValues,
    stored: &StoredConnections,
) -> Result<BTreeMap<EnvironmentVariableName, String>, NativeV2CliError> {
    let source = if let Some(values) = explicit.get(key) {
        let source = values.as_map();
        if source.len() != fields.len() || fields.iter().any(|field| !source.contains_key(field)) {
            return Err(local_message(format!(
                "explicit connection {key} does not exactly define its required fields"
            )));
        }
        source
    } else {
        stored
            .get(key)
            .map(StaticConnectionValues::as_map)
            .ok_or_else(|| local_message(format!("required connection {key} is unavailable")))?
    };
    fields
        .iter()
        .map(|field| {
            let value = source.get(field).cloned().ok_or_else(|| {
                local_message(format!(
                    "connection {key} is missing required field {field}"
                ))
            })?;
            Ok((field.clone(), value))
        })
        .collect()
}

fn temporary_path(root: &Path) -> PathBuf {
    root.join(format!("connections.{}.tmp", uuid::Uuid::now_v7()))
}

fn require_user_scope(scope: ConnectionScope) -> Result<(), NativeV2CliError> {
    if scope == ConnectionScope::User {
        Ok(())
    } else {
        Err(local_message(
            "organization-scoped connections require a hosted target",
        ))
    }
}

fn summary(key: ConnectionKey, fields: Vec<EnvironmentVariableName>) -> ConnectionSummary {
    ConnectionSummary {
        key,
        scope: ConnectionScope::User,
        kind: STATIC_CONNECTION_KIND.to_owned(),
        fields,
    }
}

#[cfg(test)]
#[path = "connections/tests.rs"]
mod tests;
