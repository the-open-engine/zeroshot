//! Cooperative cancellation shared by an async owner and its blocking conversation copy.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::execution::driver::DriverCancellation;

#[derive(Clone, Default)]
pub(crate) struct ConversationIo {
    stopped: Arc<AtomicBool>,
    request: Option<DriverCancellation>,
}

impl ConversationIo {
    pub(crate) fn with_request(&self, request: DriverCancellation) -> Self {
        Self {
            stopped: self.stopped.clone(),
            request: Some(request),
        }
    }

    pub(crate) fn stop_handle(&self) -> std::sync::Weak<AtomicBool> {
        Arc::downgrade(&self.stopped)
    }

    pub(crate) fn cancel(&self) {
        self.stopped.store(true, Ordering::Release);
    }

    pub(super) fn check(&self) -> Result<(), &'static str> {
        if self.stopped.load(Ordering::Acquire)
            || self
                .request
                .as_ref()
                .is_some_and(DriverCancellation::is_cancelled)
        {
            return Err("conversation copying was cancelled");
        }
        Ok(())
    }

    pub(crate) fn cancel_on_drop(&self) -> CancelOnDrop<'_> {
        CancelOnDrop(Some(self))
    }
}

pub(crate) struct CancelOnDrop<'a>(Option<&'a ConversationIo>);

impl CancelOnDrop<'_> {
    pub(crate) fn disarm(mut self) {
        self.0.take();
    }
}

impl Drop for CancelOnDrop<'_> {
    fn drop(&mut self) {
        if let Some(operation) = self.0 {
            operation.cancel();
        }
    }
}
