use super::*;
#[test]
fn json_envelope_accepts_512_containers_and_refuses_513_before_decoding() {
    let wrapped = format!("{}null{}", "[".repeat(512), "]".repeat(512));
    assert!(bounded_json_from_str::<serde_json::Value>(&wrapped).is_ok());
    let excessive = format!("{}null{}", "[".repeat(513), "]".repeat(513));
    assert!(
        bounded_json_from_str::<serde_json::Value>(&excessive)
            .unwrap_err()
            .to_string()
            .contains("512")
    );
    assert_eq!(
        bounded_json_from_str::<String>(r#""[{\\\"}]""#).unwrap(),
        "[{\\\"}]"
    );
    assert!(bounded_json_from_str::<serde_json::Value>("null null").is_err());
}
