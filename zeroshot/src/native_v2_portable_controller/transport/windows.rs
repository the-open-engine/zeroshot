use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions};
use tokio::sync::Mutex;

use crate::execution::platform::windows::security::{PrivateSecurity, validate};

pub(crate) type Client = NamedPipeClient;

pub(crate) struct Listener {
    path: PathBuf,
    pending: Mutex<NamedPipeServer>,
}

pub(crate) async fn connect(path: &Path) -> io::Result<Client> {
    let client = ClientOptions::new().open(path)?;
    validate(client.as_raw_handle())?;
    Ok(client)
}

impl Listener {
    pub(crate) fn bind(path: &Path) -> io::Result<Self> {
        Ok(Self {
            path: path.to_owned(),
            pending: Mutex::new(create(path, true)?),
        })
    }

    pub(crate) async fn accept(&self) -> io::Result<NamedPipeServer> {
        let mut pending = self.pending.lock().await;
        pending.connect().await?;
        // Keep a listening instance alive across accepts, preventing pipe-name takeover.
        let next = create(&self.path, false)?;
        Ok(std::mem::replace(&mut *pending, next))
    }
}

fn create(path: &Path, first: bool) -> io::Result<NamedPipeServer> {
    let descriptor = PrivateSecurity::new()?;
    let mut attributes = descriptor.attributes();
    // The descriptor and attributes outlive CreateNamedPipe; neither is retained by Windows.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(path, std::ptr::from_mut(&mut attributes).cast())
    }
}

pub(crate) fn remove_endpoint(_path: &Path) -> io::Result<()> {
    Ok(())
}
