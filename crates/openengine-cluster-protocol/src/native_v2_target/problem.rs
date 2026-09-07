use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::NativeV2RunValueError;

pub const TARGET_RUN_REJECTED_CODE: &str = "run.rejected";
pub const MAX_TARGET_HTTP_PROBLEM_CODE_BYTES: usize = 128;
pub const MAX_TARGET_HTTP_PROBLEM_MESSAGE_BYTES: usize = 1_024;
pub const MAX_TARGET_HTTP_PROBLEM_DETAILS_BYTES: usize = 60 * 1_024;

/// Public, secret-free problem returned by a target HTTP endpoint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TargetHttpProblem {
    code: String,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl TargetHttpProblem {
    pub fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Option<Value>,
    ) -> Result<Self, NativeV2RunValueError> {
        let code = code.into();
        if code.is_empty()
            || code.len() > MAX_TARGET_HTTP_PROBLEM_CODE_BYTES
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(NativeV2RunValueError(
                "target HTTP problem code must be 1..=128 ASCII alphanumeric/underscore/hyphen/dot bytes",
            ));
        }
        let message = message.into();
        if message.is_empty()
            || message.len() > MAX_TARGET_HTTP_PROBLEM_MESSAGE_BYTES
            || message.chars().any(char::is_control)
        {
            return Err(NativeV2RunValueError(
                "target HTTP problem message must be 1..=1024 non-control UTF-8 bytes",
            ));
        }
        if details.as_ref().is_some_and(|details| {
            !details.is_object()
                || serde_json::to_vec(details).map_or(true, |encoded| {
                    encoded.len() > MAX_TARGET_HTTP_PROBLEM_DETAILS_BYTES
                })
        }) {
            return Err(NativeV2RunValueError(
                "target HTTP problem details must be an object no larger than 61440 bytes",
            ));
        }
        Ok(Self {
            code,
            message,
            details,
        })
    }

    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[must_use]
    pub const fn details(&self) -> Option<&Value> {
        self.details.as_ref()
    }

    #[must_use]
    pub fn into_parts(self) -> (String, String, Option<Value>) {
        (self.code, self.message, self.details)
    }
}

impl<'de> Deserialize<'de> for TargetHttpProblem {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        struct Wire {
            code: String,
            message: String,
            #[serde(default)]
            details: Option<Value>,
        }

        let wire = Wire::deserialize(deserializer)?;
        Self::new(wire.code, wire.message, wire.details).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use openengine_cluster_testkit::assertions::AssertValue;

    use super::*;

    #[test]
    fn target_http_problem_is_bounded_structured_and_closed() {
        let problem = TargetHttpProblem::new(
            TARGET_RUN_REJECTED_CODE,
            "required input binding is missing",
            Some(serde_json::json!({"binding": "issueNumber"})),
        )
        .assert_value();
        let encoded = serde_json::to_value(&problem).assert_value();
        assert_eq!(
            encoded,
            serde_json::json!({
                "code": TARGET_RUN_REJECTED_CODE,
                "message": "required input binding is missing",
                "details": {"binding": "issueNumber"}
            })
        );
        assert!(TargetHttpProblem::new("bad code", "rejected", None).is_err());
        assert!(
            TargetHttpProblem::new(
                TARGET_RUN_REJECTED_CODE,
                "x".repeat(MAX_TARGET_HTTP_PROBLEM_MESSAGE_BYTES + 1),
                None,
            )
            .is_err()
        );
        assert!(
            TargetHttpProblem::new(
                TARGET_RUN_REJECTED_CODE,
                "rejected",
                Some(serde_json::json!(["not", "an", "object"])),
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<TargetHttpProblem>(serde_json::json!({"message": "rejected"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<TargetHttpProblem>(serde_json::json!({
                "code": TARGET_RUN_REJECTED_CODE,
                "message": "rejected",
                "extra": true
            }))
            .is_err()
        );
    }
}
