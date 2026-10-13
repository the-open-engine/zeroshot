//! Tests for `native_v2_pi/session_id.rs`.
//!
//! Kept outside the production-named source file so production coverage totals exclude them
//! (`AGENTS.md`, `COVERAGE_IGNORE_REGEX` in `.github/workflows/coverage.yml`).

use super::{agent_directory, observe_session, pi_session_id, session_directory};
use std::path::Path;

#[test]
fn derivation_is_stable_and_within_the_pi_charset() {
    let first = pi_session_id("node-instance-7").unwrap();
    assert_eq!(first, pi_session_id("node-instance-7").unwrap());
    assert_eq!(first.len(), 67);
    assert!(first.starts_with("zs-"));
    assert!(
        first
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    );
    assert!(
        first
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
    );
}

#[test]
fn distinct_identities_derive_distinct_sessions() {
    assert_ne!(
        pi_session_id("node-instance-7").unwrap(),
        pi_session_id("node-instance-8").unwrap()
    );
}

#[test]
fn identity_outside_the_supported_shape_is_rejected() {
    assert!(pi_session_id("").is_err());
    assert!(pi_session_id(&"x".repeat(513)).is_err());
    assert!(pi_session_id(&"x".repeat(512)).is_ok());
}

#[test]
fn digest_matches_the_published_sha256_vectors() {
    // Pins the charset adapter to standard SHA-256 so a future rewrite cannot silently shift
    // session identities.
    assert_eq!(
        pi_session_id("abc").unwrap(),
        "zs-ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        pi_session_id("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq").unwrap(),
        "zs-248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

#[test]
fn session_directories_stay_inside_the_private_home() {
    let home = Path::new("/run/pi-home");
    assert_eq!(agent_directory(home), home);
    assert_eq!(session_directory(home), home.join("sessions"));
}

#[test]
fn a_different_provider_session_is_a_provider_failure() {
    let expected = pi_session_id("node-instance-7").unwrap();
    assert!(observe_session(Some(&expected), &expected).is_ok());
    assert!(observe_session(None, &expected).is_ok());
    assert!(observe_session(Some("zs-other"), &expected).is_err());
    assert!(observe_session(Some(""), &expected).is_err());
}
