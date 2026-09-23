use openengine_cluster_testkit::assertions::AssertValue;
use serde_json::json;

use super::*;

#[test]
fn advertised_routes_are_same_origin_and_dynamic_kinds_are_bounded() {
    let origin = Url::parse("https://target.example").assert_value();
    let extensions = serde_json::from_value::<TargetDiscoveryExtensions>(json!({
        "connections": {
            "kind": CONNECTIONS_KIND,
            "baseUrl": "https://target.example/api/",
            "routeTemplates": {
                "list": "/connections/list",
                "set": "/connections/set",
                "delete": "/connections/delete",
                "resolve": "/connections/resolve"
            },
            "dynamicKinds": ["github_app"]
        }
    }))
    .assert_value();
    let descriptor = build_connections_descriptor(&origin, &extensions)
        .assert_value()
        .assert_value();
    assert_eq!(
        descriptor.list.as_str(),
        "https://target.example/api/connections/list"
    );
    assert_eq!(
        extensions.connections.as_ref().assert_value().dynamic_kinds,
        ["github_app"]
    );

    let mut cross_origin = extensions;
    cross_origin.connections.as_mut().assert_value().base_url =
        "https://attacker.example".to_owned();
    assert!(build_connections_descriptor(&origin, &cross_origin).is_err());
}
