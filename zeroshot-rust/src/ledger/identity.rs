use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_IDENTIFIER_BYTES: usize = 256;
pub const MAX_SQLITE_SEQUENCE: u64 = i64::MAX as u64;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum IdentityError {
    #[error("identifier must not be empty")]
    Empty,
    #[error("identifier exceeds 256 UTF-8 bytes")]
    TooLong,
    #[error("identifier contains a control character")]
    ControlCharacter,
    #[error("counter exceeds the SQLite signed-integer range")]
    CounterOverflow,
    #[error("absolute deadline must be nonzero")]
    InvalidDeadline,
}

fn validate_identifier(value: &str) -> Result<(), IdentityError> {
    if value.is_empty() {
        return Err(IdentityError::Empty);
    }
    if value.len() > MAX_IDENTIFIER_BYTES {
        return Err(IdentityError::TooLong);
    }
    if value.chars().any(char::is_control) {
        return Err(IdentityError::ControlCharacter);
    }
    Ok(())
}

macro_rules! identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, IdentityError> {
                let value = value.into();
                validate_identifier(&value)?;
                Ok(Self(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdentityError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

identifier!(ResourceId);
identifier!(OwnerId);
identifier!(IdempotencyId);
identifier!(NodeInstanceId);
identifier!(ExecutionId);
identifier!(LedgerRunId);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct LedgerGeneration(u64);

impl LedgerGeneration {
    pub fn new(value: u64) -> Result<Self, IdentityError> {
        if value == 0 || value > MAX_SQLITE_SEQUENCE {
            Err(IdentityError::CounterOverflow)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Result<Self, IdentityError> {
        Self::new(
            self.0
                .checked_add(1)
                .ok_or(IdentityError::CounterOverflow)?,
        )
    }
}

impl TryFrom<u64> for LedgerGeneration {
    type Error = IdentityError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<LedgerGeneration> for u64 {
    fn from(value: LedgerGeneration) -> Self {
        value.get()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct Position(u64);

impl Position {
    pub const ZERO: Self = Self(0);

    pub fn new(value: u64) -> Result<Self, IdentityError> {
        if value > MAX_SQLITE_SEQUENCE {
            Err(IdentityError::CounterOverflow)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    pub fn checked_next(self) -> Result<Self, IdentityError> {
        Self::new(
            self.0
                .checked_add(1)
                .ok_or(IdentityError::CounterOverflow)?,
        )
    }
}

impl TryFrom<u64> for Position {
    type Error = IdentityError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Position> for u64 {
    fn from(value: Position) -> Self {
        value.get()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct AbsoluteDeadline(u64);

impl AbsoluteDeadline {
    pub fn from_unix_millis(value: u64) -> Result<Self, IdentityError> {
        if value == 0 {
            Err(IdentityError::InvalidDeadline)
        } else if value > MAX_SQLITE_SEQUENCE {
            Err(IdentityError::CounterOverflow)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn unix_millis(self) -> u64 {
        self.0
    }
}

impl TryFrom<u64> for AbsoluteDeadline {
    type Error = IdentityError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::from_unix_millis(value)
    }
}

impl From<AbsoluteDeadline> for u64 {
    fn from(value: AbsoluteDeadline) -> Self {
        value.unix_millis()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnerFence {
    pub owner: OwnerId,
    pub epoch: u64,
    pub expires_at_unix_millis: u64,
}

impl OwnerFence {
    #[must_use]
    pub const fn is_expired_at(&self, now_unix_millis: u64) -> bool {
        self.expires_at_unix_millis <= now_unix_millis
    }
}
