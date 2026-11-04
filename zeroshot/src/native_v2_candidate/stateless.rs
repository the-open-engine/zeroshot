//! Session lifetime for stateless native bindings.
use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use async_trait::async_trait;
use crate::native_v2_runner::NodeSession;

#[derive(Default)]
pub(super) struct StatelessSession {
    closed: AtomicBool,
}

#[async_trait]
impl NodeSession for StatelessSession {
    fn as_any(&self) -> &dyn Any {
        self
    }
    async fn is_live(&self) -> bool {
        !self.closed.load(Ordering::Acquire)
    }
    async fn close(&self) {
        self.closed.store(true, Ordering::Release);
    }
}
