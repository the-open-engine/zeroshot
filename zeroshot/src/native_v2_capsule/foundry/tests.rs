use super::*;

fn values(fields: &[(&str, &str)]) -> BTreeMap<String, String> {
    fields
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn foundry_accepts_a_resource_name_or_a_root_url() {
    for resource in ["contoso-ai", "ab", "Contoso01", &"a".repeat(64)] {
        let values = values(&[(API_KEY, "sentinel-key"), (RESOURCE, resource)]);
        assert_eq!(
            connection(&values).ok(),
            Some((Endpoint::Resource(resource), "sentinel-key"))
        );
    }
    for (base, root) in [
        ("https://ai.contoso.example", "https://ai.contoso.example"),
        ("https://ai.contoso.example/", "https://ai.contoso.example"),
        (
            "https://proxy.example/foundry",
            "https://proxy.example/foundry",
        ),
    ] {
        let values = values(&[(API_KEY, "sentinel-key"), (BASE_URL, base)]);
        assert_eq!(
            connection(&values).ok(),
            Some((Endpoint::BaseUrl(root), "sentinel-key"))
        );
    }
}

#[test]
fn foundry_rejects_invalid_resources_and_secret_bearing_urls() {
    for resource in [
        "",
        "a",
        "-contoso",
        "contoso-",
        "con.toso",
        "con toso",
        &"a".repeat(65),
    ] {
        let values = values(&[(API_KEY, "sentinel-key"), (RESOURCE, resource)]);
        assert!(connection(&values).is_err(), "accepted {resource:?}");
    }
    for base in [
        "",
        "ai.contoso.example",
        "https://user:secret@ai.contoso.example",
        "https://ai.contoso.example?key=secret",
        "https://ai.contoso.example#secret",
    ] {
        let values = values(&[(API_KEY, "sentinel-key"), (BASE_URL, base)]);
        let error = connection(&values).expect_err("invalid URL accepted");
        assert!(!error.to_string().contains("secret"));
        assert!(!error.to_string().contains("sentinel-key"));
    }
}

#[test]
fn foundry_requires_a_key_and_exactly_one_endpoint() {
    for fields in [
        vec![(RESOURCE, "contoso-ai")],
        vec![(API_KEY, ""), (RESOURCE, "contoso-ai")],
        vec![(API_KEY, "key\nvalue"), (RESOURCE, "contoso-ai")],
        vec![(API_KEY, "sentinel-key")],
        vec![
            (API_KEY, "sentinel-key"),
            (RESOURCE, "contoso-ai"),
            (BASE_URL, "https://ai.contoso.example"),
        ],
    ] {
        let error = connection(&values(&fields)).expect_err("incomplete connection accepted");
        assert!(!error.to_string().contains("sentinel-key"));
    }
}
