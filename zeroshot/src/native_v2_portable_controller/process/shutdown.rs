//! One latched request for the private controller process, including preparation and retirement.

use std::time::Duration;

use tokio::sync::watch;

use super::PortableControllerError;

/// Includes cancellation, confirmed cleanup, SQLite settlement and connection draining. This is
/// independent of the async runtime: a blocking storage operation or runtime teardown cannot
/// extend it. An earlier OS deadline can still prevent cooperative settlement.
const SHUTDOWN_LIMIT: Duration = Duration::from_secs(30);

pub(super) struct ControllerShutdown {
    pub requested: watch::Receiver<bool>,
    listener: tokio::task::JoinHandle<()>,
}

impl ControllerShutdown {
    pub fn install() -> Result<Self, PortableControllerError> {
        let termination = crate::execution::platform::controller_termination()
            .map_err(PortableControllerError::Io)?;
        let (request, requested) = watch::channel(false);
        let listener = tokio::spawn(async move {
            termination.await;
            // Arm once, before requesting any work that can block. This thread deliberately
            // survives async scope/runtime teardown; normal process exit needs no join.
            if std::thread::Builder::new()
                .name("controller-shutdown-deadline".to_owned())
                .spawn(|| {
                    std::thread::sleep(SHUTDOWN_LIMIT);
                    std::process::exit(1);
                })
                .is_err()
            {
                std::process::exit(1);
            }
            request.send_replace(true);
        });
        Ok(Self {
            requested,
            listener,
        })
    }
}

impl Drop for ControllerShutdown {
    fn drop(&mut self) {
        self.listener.abort();
    }
}
