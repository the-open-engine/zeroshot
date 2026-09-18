use std::io;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::path::Path;

pub(crate) type Client = tokio::net::UnixStream;

pub(crate) struct Listener(tokio::net::UnixListener);

pub(crate) async fn connect(path: &Path) -> io::Result<Client> {
    Client::connect(path).await
}

impl Listener {
    pub(crate) fn bind(path: &Path) -> io::Result<Self> {
        remove_endpoint(path)?;
        let listener = tokio::net::UnixListener::bind(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self(listener))
    }

    pub(crate) async fn accept(&self) -> io::Result<Client> {
        self.0.accept().await.map(|(stream, _)| stream)
    }
}

pub(crate) fn remove_endpoint(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(path),
        Ok(_) => Err(io::Error::other("controller endpoint is not a socket")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
