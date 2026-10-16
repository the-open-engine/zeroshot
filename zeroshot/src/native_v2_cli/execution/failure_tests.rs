use std::time::Duration;

use super::*;
use crate::native_v2_cli::tests::support::FakeBackend;

#[tokio::test(start_paused = true)]
async fn error_log_read_ends_at_its_deadline_or_ctrl_c() {
    for (detach_after, waited) in [(60, 5), (2, 2)] {
        let backend = FakeBackend::with_pending_logs();
        let detach = tokio::time::sleep(Duration::from_secs(detach_after));
        tokio::pin!(detach);
        let started = tokio::time::Instant::now();

        let failure =
            RunFailure::read(&backend, None, RunId::new("run-local"), detach.as_mut()).await;

        assert_eq!(started.elapsed(), Duration::from_secs(waited));
        assert_eq!(
            failure.to_string(),
            "run finished unsuccessfully\nThe run log could not be read completely.\n\
             Full run log: zeroshot logs run-local"
        );
    }
}
