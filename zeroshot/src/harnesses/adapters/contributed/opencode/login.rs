//! Match native auth admission while masking only the selected provider's stored login.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_runner::NodeRunnerError;

const MAX_AUTH_BYTES: u64 = 120 * 1024;
pub(super) const SOURCE_ENV: &str = "ZEROSHOT_OPENCODE_AUTH_SOURCE";

pub(super) struct NativeLogin {
    pub(super) masked: String,
    pub(super) inline: Option<String>,
    pub(super) selected: Option<Value>,
}

impl NativeLogin {
    pub(super) fn select(
        &mut self,
        provider: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<(), NodeRunnerError> {
        if provider != "github" {
            return Ok(());
        }
        let key = environment
            .get("COPILOT_GITHUB_TOKEN")
            .ok_or(NodeRunnerError::Driver)?;
        let mut entry = json!({"type":"oauth","refresh":key,"access":key,"expires":0});
        if let Some(enterprise) = self
            .selected
            .as_ref()
            .and_then(|selected| selected.get("enterpriseUrl"))
            .filter(|value| value.is_string())
        {
            entry["enterpriseUrl"] = enterprise.clone();
        }
        let mut masked: Value =
            serde_json::from_str(&self.masked).map_err(|_| NodeRunnerError::Driver)?;
        let object = masked.as_object_mut().ok_or_else(|| {
            NodeRunnerError::DriverDetail(
                "OpenCode GitHub authentication must be an object".to_owned(),
            )
        })?;
        object.insert("github-copilot".to_owned(), entry);
        self.masked = masked.to_string();
        Ok(())
    }
}

pub(super) fn without_provider(
    source: Option<&String>,
    command: &ProcessSessionCommand,
    provider: &str,
) -> Result<NativeLogin, NodeRunnerError> {
    if source.is_some_and(|source| source.len() as u64 > MAX_AUTH_BYTES) {
        return Err(bound_error());
    }
    // Native env JSON is returned without schema admission. Invalid/empty JSON falls back to
    // auth.json, whose entries are decoded individually before native consumers see them.
    let parsed = source
        .filter(|source| !source.is_empty())
        .and_then(|source| {
            serde_json::from_str::<Value>(source)
                .ok()
                .map(|auth| (auth, source.clone()))
        });
    let (mut auth, inline) = match parsed {
        Some((auth, source)) => (auth, Some(source)),
        None => (read_auth(&auth_path(command)?)?, None),
    };
    let selected = auth.as_object_mut().and_then(|auth| auth.remove(provider));
    let auth = serde_json::to_string(&auth).map_err(|_| NodeRunnerError::Driver)?;
    if auth.len() as u64 > MAX_AUTH_BYTES {
        return Err(bound_error());
    }
    Ok(NativeLogin {
        masked: auth,
        inline,
        selected,
    })
}

fn auth_path(command: &ProcessSessionCommand) -> Result<PathBuf, NodeRunnerError> {
    let environment = &command.environment;
    let data = environment
        .get("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            environment
                .get("HOME")
                .map(|home| Path::new(home).join(".local/share"))
        })
        .ok_or(NodeRunnerError::Driver)?;
    let data = if data.is_absolute() {
        data
    } else {
        command.workspace.current_dir.join(data)
    };
    Ok(data.join("opencode/auth.json"))
}

fn read_auth(path: &Path) -> Result<Value, NodeRunnerError> {
    let Some(file) = open_auth(path)? else {
        return Ok(json!({}));
    };
    let mut bytes = Vec::new();
    if file
        .take(MAX_AUTH_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Ok(json!({}));
    }
    if bytes.len() as u64 > MAX_AUTH_BYTES {
        return Err(bound_error());
    }
    let values = serde_json::from_slice::<BTreeMap<String, Value>>(&bytes).unwrap_or_default();
    Ok(Value::Object(
        values
            .into_iter()
            .filter_map(|(name, value)| native_entry(&value).map(|entry| (name, entry)))
            .collect(),
    ))
}

fn open_auth(path: &Path) -> Result<Option<std::fs::File>, NodeRunnerError> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(None),
    };
    if !metadata.is_file() || metadata.len() > MAX_AUTH_BYTES {
        return Err(bound_error());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let metadata = file.metadata().map_err(|_| bound_error())?;
    if !metadata.is_file() || metadata.len() > MAX_AUTH_BYTES {
        return Err(bound_error());
    }
    Ok(Some(file))
}

fn native_entry(value: &Value) -> Option<Value> {
    let fields: &[&str] = match value.get("type")?.as_str()? {
        "api" if valid_api(value) => &["type", "key", "metadata"],
        "oauth" if valid_oauth(value) => &[
            "type",
            "refresh",
            "access",
            "expires",
            "accountId",
            "enterpriseUrl",
        ],
        "wellknown" if strings(value, &["key", "token"]) => &["type", "key", "token"],
        _ => return None,
    };
    admitted_fields(value, fields)
}

fn admitted_fields(value: &Value, fields: &[&str]) -> Option<Value> {
    Some(Value::Object(
        value
            .as_object()?
            .iter()
            .filter(|(name, _)| fields.contains(&name.as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    ))
}

fn valid_api(value: &Value) -> bool {
    strings(value, &["key"])
        && optional(value, "metadata", |value| {
            value
                .as_object()
                .is_some_and(|values| values.values().all(Value::is_string))
        })
}

fn valid_oauth(value: &Value) -> bool {
    strings(value, &["refresh", "access"])
        && value
            .get("expires")
            .and_then(Value::as_f64)
            .is_some_and(|value| value >= 0.0 && value.fract() == 0.0)
        && optional(value, "accountId", Value::is_string)
        && optional(value, "enterpriseUrl", Value::is_string)
}

fn strings(value: &Value, fields: &[&str]) -> bool {
    fields
        .iter()
        .all(|name| value.get(*name).is_some_and(Value::is_string))
}

fn optional(value: &Value, field: &str, valid: fn(&Value) -> bool) -> bool {
    value.get(field).is_none_or(valid)
}

fn bound_error() -> NodeRunnerError {
    NodeRunnerError::DriverDetail("OpenCode native authentication must be a bounded regular file and fit the private environment transfer".to_owned())
}
