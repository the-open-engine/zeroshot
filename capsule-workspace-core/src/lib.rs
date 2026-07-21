//! Minimal, interface-compatible core of the capsule workspace storage data plane.
//! See README.md for the drop-in mapping onto zeroshot-cloud.

pub mod cas;
pub mod daemon;
pub mod gc;
pub mod ifaces;
pub mod lineage;
pub mod manifest;
#[cfg(feature = "pg")]
pub mod pg;
pub mod refclock;
#[cfg(feature = "s3")]
pub mod s3;
