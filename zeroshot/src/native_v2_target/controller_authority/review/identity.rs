//! Keep private review snapshots within one target and login identity.
use std::io::{Read, Seek, Write};

use sha2::{Digest, Sha256};

use super::super::{TargetHttpControlAuthority, authority_error};
use crate::native_v2_target::{TargetAccess, TargetAuthorityError, TargetRecord};

impl TargetHttpControlAuthority {
    pub(in crate::native_v2_target::controller_authority) async fn review_snapshot_namespace(
        &self,
        target: &TargetRecord,
    ) -> Result<String, TargetAuthorityError> {
        let credential = match (&target.access, &self.machine) {
            (TargetAccess::Direct, _) => {
                self.require_no_machine_for_direct()?;
                std::env::var("GH_TOKEN")
                    .or_else(|_| std::env::var("GITHUB_TOKEN"))
                    .map_err(|_| authority_error("direct task resolution requires GH_TOKEN"))?
            }
            (TargetAccess::Hosted { .. }, Some(machine)) => {
                format!("{}:{:?}", machine.api_key, machine.organization)
            }
            (TargetAccess::Hosted { .. }, None) => {
                let _guard = self.lock_refresh_family(&target.id).await?;
                self.review_login_generation(&target.id, false)?
            }
        };
        namespace(target, &credential)
    }

    /// Called under the existing refresh-family lock. Token refresh preserves this identity;
    /// every login attempt rotates it before another account can replace the credential.
    pub(in crate::native_v2_target::controller_authority) fn review_login_generation(
        &self,
        target_id: &str,
        rotate: bool,
    ) -> Result<String, TargetAuthorityError> {
        super::super::credential_service(target_id)?;
        let path = self
            .refresh_lock_directory
            .join(format!("target-{target_id}-review-login"));
        let mut file = crate::native_v2_target::registry::open_lock(&path)
            .map_err(|_| authority_error("review login identity is unavailable"))?;
        let mut generation = String::new();
        if !rotate {
            (&mut file)
                .take(37)
                .read_to_string(&mut generation)
                .map_err(|_| authority_error("review login identity is unreadable"))?;
            if !generation.is_empty() {
                uuid::Uuid::parse_str(&generation)
                    .map_err(|_| authority_error("review login identity is invalid"))?;
                return Ok(generation);
            }
        }
        generation = uuid::Uuid::now_v7().to_string();
        file.rewind()
            .and_then(|()| file.set_len(0))
            .and_then(|()| file.write_all(generation.as_bytes()))
            .and_then(|()| file.sync_all())
            .map_err(|_| authority_error("review login identity could not be saved"))?;
        Ok(generation)
    }
}

fn namespace(target: &TargetRecord, credential: &str) -> Result<String, TargetAuthorityError> {
    let bytes = serde_json::to_vec(&(target, credential))
        .map_err(|_| authority_error("review cache authority is invalid"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
#[path = "identity/tests.rs"]
mod tests;
