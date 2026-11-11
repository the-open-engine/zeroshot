use super::*;
#[test]
fn wrappers_allow_depth_64_and_excessive_json_is_refused_before_decoding() {
    let wrapped = format!("{}null{}", "[".repeat(140), "]".repeat(140));
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
