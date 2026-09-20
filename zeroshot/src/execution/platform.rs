//! OS facilities shared by local controllers and in-process workers.

#[cfg(test)]
mod tests;

use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
pub(crate) mod windows;
#[cfg(unix)]
use unix as native;
#[cfg(windows)]
use windows as native;

pub(crate) use native::{
    ControllerChild, FileIdentity, commit_file, create_private_directory, file_identity,
    open_directory, open_identity, private_directory, spawn_controller,
};

#[derive(Clone, Copy)]
pub(crate) enum FileAccess {
    Read,
    ReadWrite,
    #[cfg(feature = "ui")]
    ReadWriteExisting,
    CreateNew,
}

pub(crate) fn private_file(path: &Path, access: FileAccess) -> io::Result<File> {
    native::private_file(path, access)
}

pub(crate) fn user_home() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    let variable = "USERPROFILE";
    #[cfg(not(windows))]
    let variable = "HOME";
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(Into::into)
}

/// Essential Windows runtime settings, without inheriting ambient provider credentials.
/// Homes are supplied by the adapter, including its private home for isolated sessions.
pub(crate) fn process_environment(values: &mut std::collections::BTreeMap<String, String>) {
    #[cfg(windows)]
    {
        for name in ["SystemRoot", "WINDIR", "COMSPEC", "PATHEXT", "TEMP", "TMP"] {
            if let Ok(value) = std::env::var(name) {
                if !values.keys().any(|key| key.eq_ignore_ascii_case(name)) {
                    values.insert(name.to_owned(), value);
                }
            }
        }
        if let Some(scratch) = values.get("TMPDIR").cloned() {
            values.insert("TEMP".to_owned(), scratch.clone());
            values.insert("TMP".to_owned(), scratch);
        }
        if let Some(home) = values.get("HOME").cloned() {
            values.insert("USERPROFILE".to_owned(), home.clone());
            let local = std::env::var("USERPROFILE")
                .is_ok_and(|profile| profile.eq_ignore_ascii_case(&home));
            for (name, suffix) in [("APPDATA", "Roaming"), ("LOCALAPPDATA", "Local")] {
                let directory = local
                    .then(|| std::env::var(name).ok())
                    .flatten()
                    .unwrap_or_else(|| format!("{home}\\AppData\\{suffix}"));
                values.entry(name.to_owned()).or_insert(directory);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = values;
}

/// Resolve native executables and npm's command shims through the authored search path.
/// Rust owns .cmd/.bat argument escaping; no shell command string is constructed here.
pub(crate) fn executable(
    program: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let path = std::path::Path::new(program);
        let roots = if path.components().count() > 1 {
            vec![std::path::PathBuf::new()]
        } else {
            environment
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("PATH"))
                .map(|(_, value)| std::env::split_paths(value).collect())
                .unwrap_or_default()
        };
        for root in roots {
            let candidate = root.join(path);
            let suffixes: &[&str] = if path.extension().is_some() {
                &[""]
            } else {
                &[".exe", ".com", ".cmd", ".bat"]
            };
            for suffix in suffixes {
                let mut name = candidate.as_os_str().to_owned();
                name.push(suffix);
                let candidate = std::path::PathBuf::from(name);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }
    #[cfg(not(windows))]
    let _ = environment;
    program.into()
}
