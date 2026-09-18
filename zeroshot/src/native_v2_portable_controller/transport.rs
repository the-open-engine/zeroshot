//! A user-private byte stream. Protocol framing and controller ownership remain shared.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
pub(super) use unix::*;
#[cfg(windows)]
pub(super) use windows::*;
