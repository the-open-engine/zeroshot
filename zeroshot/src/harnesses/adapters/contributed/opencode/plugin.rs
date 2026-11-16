//! Append an owned file plugin without replacing native configuration or plugin discovery.

use std::io;
use std::path::Path;

use serde_json::{Value, json};

use crate::execution::process::ProcessSessionCommand;
use crate::native_v2_capsule::provider_process::write_provider_file;

pub(super) fn install(
    command: &mut ProcessSessionCommand,
    home: &Path,
    id: &str,
    source: &str,
) -> io::Result<()> {
    let root = home.join("zeroshot-plugins");
    let plugin_dir = root.join(id);
    owned_directory(&root, home)?;
    owned_directory(&plugin_dir, home)?;
    let scratch = command
        .environment
        .get("TMPDIR")
        .ok_or_else(|| io::Error::other("TMPDIR is not set"))?;
    let marker = Path::new(scratch)
        .join("zeroshot-plugins")
        .join(format!("{id}.loaded"));
    let source = source.replace("LOADED_MARKER", &serde_json::to_string(&marker)?);
    write_provider_file(&plugin_dir, "package.json", br#"{"type":"module"}"#)?;
    write_provider_file(&plugin_dir, "index.js", source.as_bytes())?;
    let expected = root.join("expected");
    owned_directory(&expected, home)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(expected.join(id))?;
    let plugin = url::Url::from_directory_path(&plugin_dir)
        .map_err(|_| io::Error::other("OpenCode plugin path is invalid"))?;
    let mut config: Value = serde_json::from_str(
        command
            .environment
            .get("OPENCODE_CONFIG_CONTENT")
            .map(String::as_str)
            .unwrap_or("{}"),
    )?;
    let plugins = config
        .as_object_mut()
        .ok_or_else(|| io::Error::other("inline config is not an object"))?
        .entry("plugins")
        .or_insert_with(|| json!([]));
    plugins
        .as_array_mut()
        .ok_or_else(|| io::Error::other("plugins must be an array"))?
        .push(json!(plugin.as_str()));
    command.environment.insert(
        "OPENCODE_CONFIG_CONTENT".to_owned(),
        serde_json::to_string(&config)?,
    );
    Ok(())
}

/// Creates one directory below the worker-owned home, refuses links, and gives it the home's owner.
fn owned_directory(path: &Path, home: &Path) -> io::Result<()> {
    match crate::execution::platform::create_private_directory(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::other(
            "OpenCode plugin directory is not a directory",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::MetadataExt;
        let owner = std::fs::metadata(home)?;
        if metadata.uid() != owner.uid() || metadata.gid() != owner.gid() {
            std::os::unix::fs::lchown(path, Some(owner.uid()), Some(owner.gid()))?;
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = home;
    Ok(())
}

pub(super) fn reset(home: &Path, scratch: &Path) -> io::Result<()> {
    remove_dir_all_if_exists(&home.join("zeroshot-plugins").join("expected"))?;
    remove_dir_all_if_exists(&scratch.join("zeroshot-plugins"))
}

pub(super) fn conflict(scratch: &Path) -> Option<String> {
    const ACCEPTED_PREFIX: &str = "Zeroshot OpenCode declared provider conflict: ";
    const SUMMARY: &str = "Zeroshot OpenCode declared provider conflict";
    let path = scratch
        .join("zeroshot-plugins")
        .join("zeroshot-provider-auth.conflict");
    let Ok(mut file) = std::fs::File::open(&path) else {
        return None;
    };
    let mut taken = io::Read::take(&mut file, 512);
    let mut bytes = Vec::new();
    if io::Read::read_to_end(&mut taken, &mut bytes).is_err() {
        return None;
    }
    let names_only = bytes.starts_with(ACCEPTED_PREFIX.as_bytes())
        && bytes.iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b',' | b':' | b' ')
        });
    if names_only {
        return Some(String::from_utf8_lossy(&bytes).to_string());
    }
    Some(SUMMARY.to_owned())
}

pub(super) fn missing(home: &Path, scratch: &Path) -> Option<String> {
    let expected = home.join("zeroshot-plugins").join("expected");
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&expected) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                names.push(name.to_owned());
            }
        }
    }
    names.sort();
    names.into_iter().find(|name| {
        !scratch
            .join("zeroshot-plugins")
            .join(format!("{name}.loaded"))
            .exists()
    })
}

fn remove_dir_all_if_exists(path: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}
