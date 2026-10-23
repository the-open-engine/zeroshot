use super::*;
use crate::active_memory::postmortem::{PostmortemError, PostmortemRequest, RunPostmortem};
use crate::native_v2_runner::current_timestamp;

impl NativeV2Supervisor {
    pub(super) fn enabled_postmortem(
        &self,
        admitted: &AdmittedRun,
    ) -> Option<&Arc<dyn RunPostmortem>> {
        if admitted.runtime.active_memory().enabled()
            && admitted.runtime.global_agents().postmortem.enabled
            && !*self.resolution_stop.borrow()
        {
            self.postmortem.as_ref()
        } else {
            None
        }
    }

    pub(super) async fn run_postmortem(
        &self,
        admitted: &AdmittedRun,
        snapshot: &RunSnapshot,
        terminal: &TerminalResult,
    ) -> Result<(), NativeV2SupervisorError> {
        let Some(postmortem) = self.enabled_postmortem(admitted) else {
            return Ok(());
        };
        self.postmortem_log("Active Memory postmortem started".to_owned())
            .await;
        let request = PostmortemRequest {
            run_id: self.run_id.clone(),
            admitted: admitted.clone(),
            snapshot: snapshot.clone(),
            primary_terminal: terminal.clone(),
            environment: self.environment.clone(),
            ledger: self.ledger.clone(),
        };
        let (cancel, cancellation) = watch::channel(false);
        let mut stopping = self.resolution_stop.subscribe();
        let work =
            std::panic::AssertUnwindSafe(postmortem.run(request, cancellation)).catch_unwind();
        tokio::pin!(work);
        let timeout = Duration::from_millis(u64::from(
            admitted.runtime.global_agents().postmortem.timeout_ms,
        ));
        let result = tokio::select! {
            result=&mut work=>result,
            _=stopping.changed()=>{cancel.send_replace(true);work.await},
            ()=tokio::time::sleep(timeout)=>{cancel.send_replace(true);work.await},
        };
        let result = result.unwrap_or(Err(PostmortemError::Preparation));
        let message = match result {
            Ok(report) => format!(
                "Active Memory postmortem completed: {} roles completed, {} failed, {} mutations published, cancelled={}",
                report.completed_roles,
                report.failed_roles,
                report.published_mutations,
                report.cancelled
            ),
            Err(PostmortemError::CleanupUnconfirmed) => {
                self.postmortem_log(
                    "Active Memory postmortem provider cleanup could not be confirmed".to_owned(),
                )
                .await;
                return Err(NativeV2SupervisorError::CleanupUnconfirmed);
            }
            Err(error) => format!("Active Memory postmortem skipped or failed: {error}"),
        };
        self.postmortem_log(message).await;
        Ok(())
    }

    async fn postmortem_log(&self, line: String) {
        if let Ok(line) = SafeLogLine::new(line) {
            let _ = self
                .ledger
                .append(
                    &self.run_id,
                    vec![RunEvent::SafeLog {
                        execution: None,
                        timestamp: current_timestamp(),
                        stream: SafeLogStream::System,
                        line,
                        diagnostic: None,
                    }],
                )
                .await;
        }
    }
}
