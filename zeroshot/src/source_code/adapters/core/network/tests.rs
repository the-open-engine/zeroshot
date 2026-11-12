use super::*;

#[test]
fn proxy_secrets_include_password_only_and_decoded_credentials() {
    for (url, username, password) in [
        ("http://:proxy-password@proxy.invalid", "", "proxy-password"),
        (
            "proxy-user:proxy-password@proxy.invalid:8080",
            "proxy-user",
            "proxy-password",
        ),
        (":proxy-password@proxy.invalid:8080", "", "proxy-password"),
        (
            "http://proxy%2Duser:p%40ss+word@proxy.invalid",
            "proxy-user",
            "p@ss+word",
        ),
        (
            "proxy%2Duser:p%40ss+word@proxy.invalid:8080",
            "proxy-user",
            "p@ss+word",
        ),
    ] {
        assert!(
            Proxy::https(url).is_ok(),
            "fixture must be an accepted proxy form"
        );
        let secrets = proxy_secrets(url);
        assert!(secrets.contains(&password.to_owned()));
        let basic = crate::native_v2_delivery::git_auth::encode_user_password(username, password);
        assert!(secrets.contains(&basic));
        let diagnostic = crate::native_v2_delivery::command::redact(
            &format!("proxy rejected {url}: {password} Basic {basic}"),
            &secrets,
        );
        assert!(!diagnostic.contains(password), "{diagnostic}");
        assert!(!diagnostic.contains(&basic), "{diagnostic}");
    }
}

#[test]
fn empty_uppercase_values_fall_back_to_lowercase_for_every_proxy_pair() {
    for (upper, lower) in [
        ("HTTPS_PROXY", "https_proxy"),
        ("HTTP_PROXY", "http_proxy"),
        ("ALL_PROXY", "all_proxy"),
        ("NO_PROXY", "no_proxy"),
    ] {
        let network = ForgeNetwork::from_snapshot(&BTreeMap::from([
            (upper.into(), String::new()),
            (lower.into(), "caller-value".into()),
        ]));
        assert_eq!(network.value(upper, lower), Some("caller-value"));
    }
}
