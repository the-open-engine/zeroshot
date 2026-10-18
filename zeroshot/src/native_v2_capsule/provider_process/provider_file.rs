//! Replace provider configuration without following worker-created links.
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

/// Files are fully written and assigned to the private home's owner before atomic publication.
/// Linux pins that directory so a concurrent worker cannot redirect the privileged operation.
pub(crate) fn write_provider_file(home: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    if !matches!(
        Path::new(name).components().collect::<Vec<_>>().as_slice(),
        [Component::Normal(_)]
    ) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "provider filename must be one normal component",
        ));
    }
    fs::create_dir_all(home)?;
    #[cfg(target_os = "linux")]
    let directory = {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(home)?
    };
    #[cfg(target_os = "linux")]
    let pinned = {
        use std::os::fd::AsRawFd;
        PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
    };
    #[cfg(not(target_os = "linux"))]
    let pinned = home.to_path_buf();
    let mut temporary = tempfile::NamedTempFile::new_in(&pinned)?;
    temporary.write_all(bytes)?;
    #[cfg(target_os = "linux")]
    assign_owner(temporary.as_file(), &directory)?;
    temporary
        .persist(pinned.join(name))
        .map_err(|failure| failure.error)?;
    Ok(home.join(name))
}

#[cfg(target_os = "linux")]
fn assign_owner(file: &fs::File, directory: &fs::File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::MetadataExt;
    let owner = directory.metadata()?;
    let current = file.metadata()?;
    if current.uid() == owner.uid() && current.gid() == owner.gid() {
        return Ok(());
    }
    // SAFETY: the live descriptor refers to our exclusively created file; no worker path is followed.
    if unsafe { libc::fchown(file.as_raw_fd(), owner.uid(), owner.gid()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
