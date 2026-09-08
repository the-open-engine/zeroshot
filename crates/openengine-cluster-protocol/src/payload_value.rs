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

    /// Returns the effective source type after target-owned deterministic empties are added.
    #[must_use]
    pub fn materialized_subtype_of(&self, target: &Self) -> Option<Self> {
        materialized_source_type(self, target).filter(|source| source.is_subtype_of(target))
    }

    /// Adds deterministic empty values for missing required null, string, and record fields.
    pub fn materialize_missing_fields(&self, value: &mut Value) {
        let (Self::Record { fields }, Value::Object(object)) = (self, value) else {
            return;
        };
        for (name, field) in fields {
            if let Some(value) = object.get_mut(name.as_str()) {
                field.value_type.materialize_missing_fields(value);
                continue;
            }
            if field.required {
                if let Some(value) = implicit_empty_value(&field.value_type) {
                    object.insert(name.as_str().to_owned(), value);
                }
            }
        }
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

fn materialized_source_type(source: &PayloadType, target: &PayloadType) -> Option<PayloadType> {
    let (PayloadType::Record { fields: source }, PayloadType::Record { fields: target }) =
        (source, target)
    else {
        return source.is_subtype_of(target).then(|| source.clone());
    };
    materialized_record_fields(source, target).map(|fields| PayloadType::Record { fields })
}

fn materialized_record_fields(
    source: &BTreeMap<FieldName, RecordField>,
    target: &BTreeMap<FieldName, RecordField>,
) -> Option<BTreeMap<FieldName, RecordField>> {
    let mut materialized = source.clone();
    for (name, target_field) in target {
        materialize_record_field(&mut materialized, name, target_field)?;
    }
    Some(materialized)
}

fn materialize_record_field(
    materialized: &mut BTreeMap<FieldName, RecordField>,
    name: &FieldName,
    target: &RecordField,
) -> Option<()> {
    let Some(source) = materialized.get_mut(name) else {
        if target.required {
            implicit_empty_value(&target.value_type)?;
            materialized.insert(name.clone(), target.clone());
        }
        return Some(());
    };
    let materialized_type = materialized_source_type(&source.value_type, &target.value_type)?;
    if target.required && !source.required {
        implicit_empty_value(&target.value_type)?;
        source.required = true;
        source.value_type = target.value_type.clone();
    } else {
        source.value_type = materialized_type;
    }
    Some(())
}

fn implicit_empty_value(payload: &PayloadType) -> Option<Value> {
    match payload {
        PayloadType::Null => Some(Value::Null),
        PayloadType::String => Some(Value::String(String::new())),
        PayloadType::Record { fields } => {
            let mut object = serde_json::Map::new();
            for (name, field) in fields {
                if field.required {
                    object.insert(
                        name.as_str().to_owned(),
                        implicit_empty_value(&field.value_type)?,
                    );
                }
            }
            Some(Value::Object(object))
        }
        PayloadType::Boolean
        | PayloadType::Integer
        | PayloadType::Number
        | PayloadType::Array { .. }
        | PayloadType::Enum { .. } => None,
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
