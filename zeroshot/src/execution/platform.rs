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
    open_directory, open_identity, open_readonly_file, private_directory, spawn_controller,
};

#[derive(Clone, Copy)]
pub(crate) enum FileAccess {
    Read,
    ReadWrite,
    #[cfg(feature = "ui")]
    ReadWriteExisting,
    CreateNew,
}

impl FileAccess {
    fn repairs_security(self) -> bool {
        matches!(self, Self::ReadWrite | Self::CreateNew)
    }
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
        windows_search(program, environment).unwrap_or_else(|| program.into())
    }
    #[cfg(not(windows))]
    {
        let _ = environment;
        program.into()
    }
}

/// Finds the file that spawning `program` in `working_directory` would run, without spawning it.
/// On Windows `PATHEXT` is ignored, so this matches what [`executable`] spawns.
/// On Unix, empty and relative `PATH` entries resolve against `working_directory`, as in the child.
pub(crate) fn find_executable(
    program: &str,
    environment: &std::collections::BTreeMap<String, String>,
    working_directory: &Path,
) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        let _ = working_directory;
        windows_search(program, environment)
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let runnable = |candidate: &std::path::Path| {
            std::fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && (metadata.permissions().mode() & 0o111) != 0
            })
        };
        if program.contains(std::path::is_separator) {
            let candidate = working_directory.join(program);
            return runnable(&candidate).then_some(candidate);
        }
        search_directories(environment.get("PATH")?, working_directory)
            .map(|directory| directory.join(program))
            .find(|candidate| runnable(candidate))
    }
}

#[cfg(windows)]
fn windows_search(
    program: &str,
    environment: &std::collections::BTreeMap<String, String>,
) -> Option<std::path::PathBuf> {
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
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn search_directories(
    path: &str,
    working_directory: &Path,
) -> impl Iterator<Item = std::path::PathBuf> {
    std::env::split_paths(path).map(move |entry| working_directory.join(entry))
}
