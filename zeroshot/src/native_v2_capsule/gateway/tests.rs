use super::*;

#[test]
fn gateway_preserves_base_paths_and_rejects_invalid_or_secret_bearing_urls() {
    for base in [
        "https://gateway.example/api/v1",
        "HTTPS://gateway.example/API/v1",
        "hTtP://localhost:8080/anthropic",
        "http://localhost:8080/anthropic/",
        "https://gateway.example/a%2Fb",
        "https://gateway.example/quoted\"path",
    ] {
        assert!(validate_base_url(base).is_ok(), "rejected {base}");
        let values = BTreeMap::from([
            (BASE_URL.to_owned(), base.to_owned()),
            (API_KEY.to_owned(), "sentinel-key".to_owned()),
        ]);
        assert_eq!(connection(&values).ok(), Some((base, "sentinel-key")));
    }
    for base in [
        "",
        "/api/v1",
        "ftp://gateway.example",
        "https:gateway.example",
        "https://user:secret@gateway.example",
        "https://user@gateway.example",
        "https://gateway.example?key=secret",
        "https://gateway.example#secret",
        "https://gateway.example/white space",
        "https://gateway.example/\nsecret",
        "https://gateway.example\\path",
    ] {
        let error = validate_base_url(base).expect_err("invalid URL accepted");
        assert!(!error.to_string().contains("secret"));
    }
    assert!(validate_base_url(&format!("https://gateway.example/{}", "x".repeat(8192))).is_err());
}

#[test]
fn gateway_requires_both_non_empty_fields_without_control_characters() {
    for field in [BASE_URL, API_KEY] {
        for invalid in [
            None,
            Some(""),
            Some(" "),
            Some("secret\nvalue"),
            Some("secret\0value"),
        ] {
            let mut values = BTreeMap::from([
                (BASE_URL.to_owned(), "https://gateway.example".to_owned()),
                (API_KEY.to_owned(), "sentinel-key".to_owned()),
            ]);
            values.remove(field);
            if let Some(value) = invalid {
                values.insert(field.to_owned(), value.to_owned());
            }
            let error = connection(&values).expect_err("invalid connection accepted");
            assert!(error.to_string().contains(field));
            assert!(!error.to_string().contains("sentinel-key"));
        }
    }
}
