use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub(crate) type Client = tokio::net::UnixStream;

pub(crate) struct Listener {
    inner: tokio::net::UnixListener,
    path: PathBuf,
    identity: (u64, u64),
    temporary_directory: Option<PathBuf>,
}

pub(crate) async fn connect(path: &Path) -> io::Result<Client> {
    validate_socket_directory(path)?;
    Client::connect(path).await
}

impl Listener {
    pub(crate) fn bind(path: &Path) -> io::Result<Self> {
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("socket has no directory"))?;
        match crate::execution::platform::create_private_directory(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        remove_endpoint(path)?;
        let inner = tokio::net::UnixListener::bind(path)?;
        let metadata = std::fs::symlink_metadata(path)?;
        let listener = Self {
            inner,
            path: path.to_owned(),
            identity: (metadata.dev(), metadata.ino()),
            temporary_directory: path
                .file_name()
                .is_some_and(|name| name == "socket")
                .then(|| directory.to_owned()),
        };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    pub(crate) async fn accept(&self) -> io::Result<Client> {
        self.inner.accept().await.map(|(stream, _)| stream)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        if validate_socket_directory(&self.path).is_ok()
            && let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && metadata.file_type().is_socket()
            && (metadata.dev(), metadata.ino()) == self.identity
        {
            let _ = std::fs::remove_file(&self.path);
            if let Some(directory) = &self.temporary_directory {
                let _ = std::fs::remove_dir(directory);
            }
        }
    }
}

fn validate_socket_directory(path: &Path) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("socket has no directory"))?;
    let directory = crate::execution::platform::open_directory(directory)?;
    crate::execution::platform::validate_private_file(&directory)
}

pub(crate) fn remove_endpoint(path: &Path) -> io::Result<()> {
    match validate_socket_directory(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    remove_socket_file(path)
}

fn remove_socket_file(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(path),
        Ok(_) => Err(io::Error::other("controller endpoint is not a socket")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests;
