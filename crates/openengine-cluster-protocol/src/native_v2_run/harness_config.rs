use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::borrow::Cow;

use super::NativeV2RunValueError;

/// Maximum encoded size of one secret-free native configuration envelope.
pub const MAX_HARNESS_CONFIG_BYTES: usize = 64 * 1024;

/// Per-node native settings. Adapters own supported carriers and reserved controls;
/// native harnesses own option names, values and unknown-key semantics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Value", into = "Value")]
pub struct HarnessConfiguration(Map<String, Value>);

impl HarnessConfiguration {
    pub fn new(value: Value) -> Result<Self, NativeV2RunValueError> {
        let Value::Object(object) = value else {
            return Err(NativeV2RunValueError("harnessConfig must be an object"));
        };
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "args" | "options"))
        {
            return Err(NativeV2RunValueError(
                "harnessConfig supports only args and options",
            ));
        }
        if object.get("args").is_some_and(|args| {
            args.as_array().is_none_or(|args| {
                args.iter()
                    .any(|arg| arg.as_str().is_none_or(|arg| arg.contains('\0')))
            })
        }) {
            return Err(NativeV2RunValueError(
                "harnessConfig.args must be an array of NUL-free argument strings",
            ));
        }
        if object
            .get("options")
            .is_some_and(|options| !options.is_object())
        {
            return Err(NativeV2RunValueError(
                "harnessConfig.options must be an object",
            ));
        }
        if serde_json::to_vec(&object)
            .map_err(|_| NativeV2RunValueError("harnessConfig cannot be encoded"))?
            .len()
            > MAX_HARNESS_CONFIG_BYTES
        {
            return Err(NativeV2RunValueError(
                "harnessConfig exceeds 65536 encoded bytes",
            ));
        }
        Ok(Self(object))
    }

    #[must_use]
    pub fn args(&self) -> Option<Vec<String>> {
        self.0.get("args").and_then(Value::as_array).map(|args| {
            args.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
    }

    #[must_use]
    pub fn options(&self) -> Option<&Map<String, Value>> {
        self.0.get("options").and_then(Value::as_object)
    }
}

impl TryFrom<Value> for HarnessConfiguration {
    type Error = NativeV2RunValueError;
    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<HarnessConfiguration> for Value {
    fn from(value: HarnessConfiguration) -> Self {
        Self::Object(value.0)
    }
}

impl JsonSchema for HarnessConfiguration {
    fn schema_name() -> Cow<'static, str> {
        "HarnessConfiguration".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type":"object", "additionalProperties":false,
            "description":"Secret-free native configuration, at most 65536 encoded bytes. Supported carriers and reserved controls belong to the selected harness.",
            "properties":{
                "args":{"type":"array","items":{"type":"string"}},
                "options":{"type":"object","additionalProperties":true}
            }
        })
    }
}
