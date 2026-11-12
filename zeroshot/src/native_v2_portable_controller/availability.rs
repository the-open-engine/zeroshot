//! Bounded, read-only availability evidence shared by CLI inspection and local UI history.
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use openengine_cluster_client::ClusterClient;
use openengine_cluster_protocol::{
    ControllerObservation, ControllerState, RunId, RunStatusParams, RunStatusResult,
    UnixTimestampMillis,
};

use super::{
    ControllerLease, PortableControllerPaths, PortableControllerReady, PortableControllerTransport,
    connect_transport, read_ready,
};
use crate::execution::platform;

const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

pub(crate) enum ControllerAvailability {
    Responding(Arc<PortableControllerTransport>, Box<RunStatusResult>),
    Embedded(ControllerObservation),
    Unavailable(ControllerObservation),
}

pub(crate) async fn check_controller(
    paths: &PortableControllerPaths,
    id: &RunId,
) -> ControllerAvailability {
    let ready = read_ready(paths).ok().filter(|ready| ready.run_id == *id);
    let before = ControllerLease::probe(&paths.lease()).ok();
    if ready.is_some() {
        let response = tokio::time::timeout(PROBE_TIMEOUT, async {
            let transport = connect_transport(paths).await.ok()?;
            let status = ClusterClient::new(transport.as_ref())
                .run_status(RunStatusParams { run_id: id.clone() })
                .await
                .ok()?;
            (status.run_id == *id).then_some((transport, status))
        })
        .await;
        if let Ok(Some((transport, status))) = response {
            return ControllerAvailability::Responding(transport, Box::new(status));
        }
    } else if before.is_some_and(|lease| lease.held)
        && ControllerLease::probe(&paths.acp_turn_lease()).is_ok_and(|lease| lease.held)
    {
        return ControllerAvailability::Embedded(ControllerObservation {
            state: ControllerState::Unknown,
            observed_at: observed_at(),
        });
    }
    let lost = ready
        .as_ref()
        .is_some_and(|ready| confirmed_absence(paths, ready, before));
    ControllerAvailability::Unavailable(ControllerObservation {
        state: if lost {
            ControllerState::Lost
        } else {
            ControllerState::Unknown
        },
        observed_at: observed_at(),
    })
}

/// Connects a control request to the endpoint named by `id`'s readiness record. No status reply
/// is asked for first: a live controller stops owned work before it can read a busy ledger.
pub(crate) async fn connect_controller(
    paths: &PortableControllerPaths,
    id: &RunId,
) -> Option<Arc<PortableControllerTransport>> {
    read_ready(paths).ok().filter(|ready| ready.run_id == *id)?;
    tokio::time::timeout(PROBE_TIMEOUT, connect_transport(paths))
        .await
        .ok()?
        .ok()
}

fn confirmed_absence(
    paths: &PortableControllerPaths,
    ready: &PortableControllerReady,
    before: Option<super::lease::LeaseProbe>,
) -> bool {
    let Some(lease) = before else {
        return false;
    };
    // A missing endpoint, a live/reused PID, or a replaced lease cannot prove death. The
    // readiness record binds the original lease identity; legacy records remain unknown.
    !lease.held
        && ready.lease_identity == Some(lease.identity)
        && platform::process_exists(ready.pid).is_ok_and(|exists| !exists)
        && ControllerLease::probe(&paths.lease()).is_ok_and(|after| after == lease)
        && read_ready(paths).is_ok_and(|after| after == *ready)
}

fn observed_at() -> UnixTimestampMillis {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(1);
    u64::try_from(millis)
        .ok()
        .and_then(|millis| UnixTimestampMillis::new(millis).ok())
        .unwrap_or(UnixTimestampMillis::MIN)
}

#[cfg(test)]
pub(crate) mod tests;
