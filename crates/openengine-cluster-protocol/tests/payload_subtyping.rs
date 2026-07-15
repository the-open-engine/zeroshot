use std::collections::BTreeMap;

use openengine_cluster_protocol::{EnumLabel, FieldName, NonEmptyEnumSet, PayloadType, RecordField};

fn field(kind: PayloadType, required: bool) -> RecordField {
    RecordField {
        value_type: kind,
        required,
    }
}

fn record(fields: &[(&str, PayloadType, bool)]) -> PayloadType {
    PayloadType::Record {
        fields: fields
            .iter()
            .map(|(name, kind, required)| {
                (
                    FieldName::new(*name).unwrap(),
                    field(kind.clone(), *required),
                )
            })
            .collect::<BTreeMap<_, _>>(),
    }
}

fn enumeration(values: &[&str]) -> PayloadType {
    PayloadType::Enum {
        values: NonEmptyEnumSet::new(
            values
                .iter()
                .map(|value| EnumLabel::new(*value).unwrap())
                .collect(),
        )
        .unwrap(),
    }
}

#[test]
fn closed_payload_subtyping_matches_the_normative_rules() {
    for primitive in [
        PayloadType::Null,
        PayloadType::Boolean,
        PayloadType::Integer,
        PayloadType::Number,
        PayloadType::String,
    ] {
        assert!(primitive.is_subtype_of(&primitive));
    }
    assert!(PayloadType::Integer.is_subtype_of(&PayloadType::Number));
    assert!(!PayloadType::Number.is_subtype_of(&PayloadType::Integer));
    assert!(!PayloadType::String.is_subtype_of(&PayloadType::Boolean));

    let integer_array = PayloadType::Array {
        items: Box::new(PayloadType::Integer),
    };
    let number_array = PayloadType::Array {
        items: Box::new(PayloadType::Number),
    };
    assert!(integer_array.is_subtype_of(&number_array));
    assert!(!number_array.is_subtype_of(&integer_array));

    assert!(enumeration(&["accepted"]).is_subtype_of(&enumeration(&["rejected", "accepted"])));
    assert!(!enumeration(&["accepted", "other"]).is_subtype_of(&enumeration(&["accepted"])));

    let narrow = record(&[
        ("id", PayloadType::Integer, true),
        (
            "nested",
            record(&[("value", integer_array.clone(), true)]),
            true,
        ),
        ("extra", PayloadType::String, true),
    ]);
    let wide = record(&[
        ("id", PayloadType::Number, true),
        (
            "nested",
            record(&[("value", number_array.clone(), true)]),
            true,
        ),
        ("optional", PayloadType::String, false),
    ]);
    assert!(narrow.is_subtype_of(&wide));
    assert!(!wide.is_subtype_of(&narrow));

    assert!(
        record(&[("x", PayloadType::String, true)]).is_subtype_of(&record(&[(
            "x",
            PayloadType::String,
            false
        )]))
    );
    assert!(
        !record(&[("x", PayloadType::String, false)]).is_subtype_of(&record(&[(
            "x",
            PayloadType::String,
            true
        )]))
    );
    assert!(record(&[]).is_subtype_of(&record(&[("x", PayloadType::String, false)])));
    assert!(!record(&[]).is_subtype_of(&record(&[("x", PayloadType::String, true)])));
}

#[test]
fn enum_sets_are_canonical_and_wire_input_rejects_empty_or_duplicate_values() {
    let values = NonEmptyEnumSet::new(vec![
        EnumLabel::new("z").unwrap(),
        EnumLabel::new("a").unwrap(),
        EnumLabel::new("z").unwrap(),
    ])
    .unwrap();
    assert_eq!(
        serde_json::to_value(values).unwrap(),
        serde_json::json!(["a", "z"])
    );
    assert!(
        serde_json::from_value::<PayloadType>(serde_json::json!({
            "kind": "enum", "values": []
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<PayloadType>(serde_json::json!({
            "kind": "enum", "values": ["a", "a"]
        }))
        .is_err()
    );
}
