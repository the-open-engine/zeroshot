//! Bounded derivation of Pi session identifiers.
//!
//! Pi requires a session identifier to start and end with an alphanumeric character and to stay
//! within `[A-Za-z0-9._-]`. Zeroshot's node-instance and execution identities are bounded opaque
//! strings with no such guarantee, so they are never passed through unchanged: each is hashed into
//! the Pi charset. The derivation is stable, which is what lets a node-instance session survive a
//! loop revisit and reuse the same provider session.

use sha2::{Digest, Sha256};

/// Longest identity this module will hash. Anything larger is rejected rather than truncated, so
/// two distinct identities cannot collide by sharing a prefix.
const MAX_IDENTITY_BYTES: usize = 512;

/// Derives a Pi session identifier from one Zeroshot identity.
///
/// The `zs-` prefix plus 64 hex characters is 67 characters and always begins and ends
/// alphanumeric, which satisfies Pi's documented constraint.
pub(super) fn pi_session_id(identity: &str) -> Result<String, &'static str> {
    if identity.is_empty() {
        return Err("Pi session identity must not be empty");
    }
    if identity.len() > MAX_IDENTITY_BYTES {
        return Err("Pi session identity exceeds the supported length");
    }
    Ok(format!("zs-{}", hex_digest(identity.as_bytes())))
}

/// Session storage directory for one provider session home.
pub(super) fn session_directory(home: &std::path::Path) -> std::path::PathBuf {
    home.join("sessions")
}

/// Agent directory for one provider session home.
pub(super) fn agent_directory(home: &std::path::Path) -> std::path::PathBuf {
    home.to_owned()
}

/// Lowercase hex SHA-256 of one bounded identity. Used only as a charset adapter: the input is
/// already a unique private identity, never a secret, so this carries no security claim beyond the
/// digest itself.
fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Reads the identity Pi reports in its session header and checks it against the derived value.
///
/// A mismatch means Pi is not addressing the session Zeroshot derived, which would silently split
/// one logical node session across two provider sessions.
pub(super) fn observe_session(observed: Option<&str>, expected: &str) -> Result<(), &'static str> {
    let Some(observed) = observed else {
        return Ok(());
    };
    if observed.is_empty() || observed.contains('\0') {
        return Err("Pi output contained an invalid session identifier");
    }
    if observed == expected {
        Ok(())
    } else {
        Err("Pi output reported a different session than the admitted node session")
    }
}

#[cfg(test)]
#[path = "session_id/tests.rs"]
mod tests;
