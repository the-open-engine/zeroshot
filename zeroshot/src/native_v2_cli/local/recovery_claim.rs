//! The per-run recovery claim: steers share it, a successor's preparation and force-stop hold it alone.
use super::*;

// A steer holds the claim for moments; only a successor's preparation holds it longer.
pub(super) const RECOVERY_CLAIM_WAIT: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    Duration::from_secs(2)
};
// Resume waits under the shared submission lock, which a competing resume needs to publish.
pub(super) const RECOVERY_CLAIM_HANDOFF: Duration = Duration::from_millis(250);

impl LocalCliBackend {
    pub(super) fn try_recovery_claim(
        &self,
        run_id: &RunId,
        shared: bool,
    ) -> Result<Option<LocalRecoveryClaim>, NativeV2CliError> {
        let path = self.recovery_claim_path(run_id)?;
        let claim = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                local_message(format!(
                    "workspace claim {} could not be opened: {error}",
                    path.display()
                ))
            })?;
        let locked = if shared {
            FileExt::try_lock_shared(&claim)
        } else {
            claim.try_lock_exclusive()
        };
        match locked {
            Ok(()) => Ok(Some(LocalRecoveryClaim(claim))),
            Err(error) if error.kind() == fs2::lock_contended_error().kind() => Ok(None),
            Err(error) => Err(local_message(format!(
                "workspace claim {} could not be acquired: {error}",
                path.display()
            ))),
        }
    }

    /// Steers share the claim: they exclude a successor's preparation, never each other, and
    /// status still reads the workspace as unclaimed. Resume and force-stop take it alone.
    /// Behind steers alone a caller waits `RECOVERY_CLAIM_WAIT` at least, since they hold the
    /// claim for moments; `wait` bounds a wait behind another resume or a force-stop.
    pub(super) async fn claim_recovery_workspace_within(
        &self,
        run_id: &RunId,
        shared: bool,
        wait: Duration,
    ) -> Result<LocalRecoveryClaim, NativeV2CliError> {
        let started = Instant::now();
        loop {
            if let Some(claim) = self.try_recovery_claim(run_id, shared)? {
                return Ok(claim);
            }
            let steers_only = !shared && self.try_recovery_claim(run_id, true)?.is_some();
            let bound = if steers_only {
                wait.max(RECOVERY_CLAIM_WAIT)
            } else {
                wait
            };
            if started.elapsed() >= bound {
                return Err(local_message(format!(
                    "retained workspace is already being resumed (claim {})",
                    self.recovery_claim_path(run_id)?.display()
                )));
            }
            sleep(CONTROLLER_HANDOFF_RETRY_DELAY).await;
        }
    }

    #[cfg(test)]
    pub(super) fn claim_recovery_workspace(
        &self,
        run_id: &RunId,
    ) -> Result<LocalRecoveryClaim, NativeV2CliError> {
        let path = self.recovery_claim_path(run_id)?;
        self.try_recovery_claim(run_id, false)?.ok_or_else(|| {
            local_message(format!(
                "retained workspace is already being resumed (claim {})",
                path.display()
            ))
        })
    }
}
