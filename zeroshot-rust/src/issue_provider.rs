//! Secret-free contracts for issue providers.
//!
//! Issue operation identifiers cannot enter the source operation namespace:
//!
//! ```compile_fail
//! use zeroshot_engine::issue_provider::IssueOperationId;
//! use zeroshot_engine::source_code_provider::SourceOperationId;
//!
//! let issue = IssueOperationId::new("close-17").unwrap();
//! let source: SourceOperationId = issue;
//! ```
//!
//! Canonical issue and repository identities remain distinct:
//!
//! ```compile_fail
//! use zeroshot_engine::issue_provider::IssueId;
//! use zeroshot_engine::source_code_provider::SourceRepositoryId;
//!
//! let issue = IssueId::new("ENG-17").unwrap();
//! let repository: SourceRepositoryId = issue;
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde::{ser, Deserialize, Serialize, Serializer};
use thiserror::Error;

use crate::provider_value::{
    bounded_bytes_type, bounded_text_type, profile_descriptor_type, provider_contract_types,
    provider_descriptor_type, validate_serialized, BoundedVec,
};
use crate::source_code_provider::SourceMergeReceipt;

const PROFILE_ID_MAX: usize = 128;
const EXTERNAL_ID_MAX: usize = 256;
const PUBLIC_URL_MAX: usize = 2_048;

provider_contract_types!(
    IssueContractError,
    IssueProviderId,
    IssueProfileId,
    IssueAccountId,
    IssueCredentialHandleId,
    IssueOperationId,
    IssueOperationFingerprint,
    IssueProviderRef,
    PROFILE_ID_MAX,
    "issue"
);
bounded_text_type!(
    IssueReference,
    EXTERNAL_ID_MAX,
    IssueContractError,
    "issue reference"
);
bounded_text_type!(IssueId, EXTERNAL_ID_MAX, IssueContractError, "issue id");
bounded_bytes_type!(
    IssuePublicUrl,
    PUBLIC_URL_MAX,
    IssueContractError,
    "public URL"
);
bounded_text_type!(
    IssueFailureMessage,
    EXTERNAL_ID_MAX,
    IssueContractError,
    "failure message"
);
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCapability {
    Read,
    Close,
}

profile_descriptor_type!(
    IssueProfileDescriptor,
    IssueProfileDescriptorWire,
    "IssueProfileDescriptorWire",
    IssueCapability,
    IssueContractError,
    "issue capabilities"
);

provider_descriptor_type!(
    IssueProviderDescriptor,
    IssueProviderDescriptorWire,
    "IssueProviderDescriptorWire",
    IssueProviderRef,
    IssueProfileId,
    IssueProfileDescriptor,
    IssueContractError,
    "issue profiles"
);

mod contracts;
mod evidence;
mod registry;

pub use contracts::*;
pub use evidence::*;
pub use registry::*;
