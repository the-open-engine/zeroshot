//! The access check. It observes the authenticated user, then the server version and the
//! repository read and write permissions, and never writes to the forge. A stop request ends the
//! check at any point, including dropping a request that is already in flight, and a stopped check
//! is an error rather than a result.

use tokio::sync::watch;

use crate::execution::process::wait_for_stop_request;
use crate::source_code_provider::{SourceDeliveryCredential, SourceDeliveryError};

use super::api::ApiRequest;
use super::authority::ForgejoDeliveryAuthority;
use super::wire;

const STOPPED: &str = "Forgejo repository access request was stopped before it completed";
const NO_USER: &str = "Forgejo authentication check failed: token did not identify a user";
const NO_READ: &str = "Forgejo repository read failed: token cannot read this repository";
const NO_WRITE: &str = "Forgejo repository write failed: token cannot push to this repository";

pub(super) async fn check(
    authority: &ForgejoDeliveryAuthority,
    repository: &str,
    credential: SourceDeliveryCredential<'_>,
    stopping: Option<watch::Receiver<bool>>,
) -> Result<(), SourceDeliveryError> {
    // A stop request drops every pending HTTP request; this client starts no process to clean up.
    tokio::select! {
        biased;
        () = wait_for_stop_request(stopping) => Err(SourceDeliveryError::api(None, STOPPED)),
        result = observe(authority, repository, credential) => result,
    }
}

async fn observe(
    authority: &ForgejoDeliveryAuthority,
    repository: &str,
    credential: SourceDeliveryCredential<'_>,
) -> Result<(), SourceDeliveryError> {
    let user: wire::User = authority
        .api
        .json(ApiRequest::get(&["user"]), credential)
        .await?;
    if user.login.is_empty() {
        return Err(SourceDeliveryError::api(Some(401), NO_USER));
    }
    let observed = authority.repository(repository, credential).await?;
    let permissions = observed
        .permissions
        .as_ref()
        .ok_or_else(|| SourceDeliveryError::identity(NO_READ))?;
    if !permissions.pull {
        return Err(SourceDeliveryError::identity(NO_READ));
    }
    if !permissions.push {
        return Err(SourceDeliveryError::identity(NO_WRITE));
    }
    Ok(())
}
