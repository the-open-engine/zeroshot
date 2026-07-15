//! Total JSON value validation for the closed payload algebra.

use serde_json::Value;
use thiserror::Error;

use crate::{EnumLabel, FieldName, NonEmptyEnumSet, PayloadType, RecordField};
use std::collections::BTreeMap;

impl PayloadType {
    /// Validate a JSON value without delegating to JSON Schema.
    ///
    /// Records are closed: undeclared fields are rejected.
    pub fn validate_value(&self, value: &Value) -> Result<(), PayloadValueError> {
        self.validate_value_at(value, "$".to_owned())
    }

    fn validate_value_at(&self, value: &Value, path: String) -> Result<(), PayloadValueError> {
        match self {
            Self::Null => expect_type(value.is_null(), path, "null"),
            Self::Boolean => expect_type(value.is_boolean(), path, "boolean"),
            Self::Integer => expect_type(is_json_integer(value), path, "integer"),
            Self::Number => expect_type(value.as_f64().is_some_and(f64::is_finite), path, "number"),
            Self::String => expect_type(value.is_string(), path, "string"),
            Self::Enum { values } => validate_enum(values, value, path),
            Self::Array { items } => validate_array(items, value, path),
            Self::Record { fields } => validate_record(fields, value, path),
        }
    }
}

fn expect_type(
    matches: bool,
    path: String,
    expected: &'static str,
) -> Result<(), PayloadValueError> {
    matches
        .then_some(())
        .ok_or(PayloadValueError::TypeMismatch { path, expected })
}

fn validate_enum(
    values: &NonEmptyEnumSet,
    value: &Value,
    path: String,
) -> Result<(), PayloadValueError> {
    let label = value
        .as_str()
        .ok_or_else(|| PayloadValueError::TypeMismatch {
            path: path.clone(),
            expected: "enum",
        })?;
    if values
        .values()
        .iter()
        .map(EnumLabel::as_str)
        .any(|value| value == label)
    {
        Ok(())
    } else {
        Err(PayloadValueError::UnknownEnumLabel {
            path,
            value: label.to_owned(),
        })
    }
}

fn validate_array(
    items: &PayloadType,
    value: &Value,
    path: String,
) -> Result<(), PayloadValueError> {
    let values = value
        .as_array()
        .ok_or_else(|| PayloadValueError::TypeMismatch {
            path: path.clone(),
            expected: "array",
        })?;
    values
        .iter()
        .enumerate()
        .try_for_each(|(index, value)| items.validate_value_at(value, format!("{path}[{index}]")))
}

fn validate_record(
    fields: &BTreeMap<FieldName, RecordField>,
    value: &Value,
    path: String,
) -> Result<(), PayloadValueError> {
    let object = value
        .as_object()
        .ok_or_else(|| PayloadValueError::TypeMismatch {
            path: path.clone(),
            expected: "record",
        })?;
    for (name, field) in fields {
        validate_record_field(name, field, object.get(name.as_str()), &path)?;
    }
    if let Some(name) = object
        .keys()
        .find(|name| !fields.keys().any(|field| field.as_str() == name.as_str()))
    {
        return Err(PayloadValueError::UnknownField {
            path,
            field: name.clone(),
        });
    }
    Ok(())
}

fn validate_record_field(
    name: &FieldName,
    field: &RecordField,
    value: Option<&Value>,
    path: &str,
) -> Result<(), PayloadValueError> {
    match value {
        Some(value) => field
            .value_type
            .validate_value_at(value, format!("{path}.{}", name.as_str())),
        None if field.required => Err(PayloadValueError::MissingRequiredField {
            path: path.to_owned(),
            field: name.as_str().to_owned(),
        }),
        None => Ok(()),
    }
}

fn is_json_integer(value: &Value) -> bool {
    value.is_i64()
        || value.is_u64()
        || value
            .as_f64()
            .is_some_and(|number| number.is_finite() && number.fract() == 0.0)
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum PayloadValueError {
    #[error("{path} must be a {expected}")]
    TypeMismatch {
        path: String,
        expected: &'static str,
    },
    #[error("{path} is missing required field {field}")]
    MissingRequiredField { path: String, field: String },
    #[error("{path} contains undeclared field {field}")]
    UnknownField { path: String, field: String },
    #[error("{path} contains unknown enum label {value}")]
    UnknownEnumLabel { path: String, value: String },
}
