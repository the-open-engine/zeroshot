use serde_json::json;

use super::*;

#[test]
fn renderer_projects_an_unknown_method_from_openrpc() {
    let document = json!({
        "openrpc": "1.3.2",
        "info": { "title": "Fixture Protocol", "version": "7" },
        "methods": [{
            "name": "fixture/custom",
            "paramStructure": "by-name",
            "params": [{
                "name": "payload",
                "required": true,
                "schema": { "$ref": "schema.json#/$defs/FixturePayload" }
            }],
            "result": {
                "name": "fixtureResult",
                "schema": { "type": ["string", "null"] }
            },
            "x-subscription": true,
            "x-transport-requirements": {
                "serverPush": true,
                "inboundNotifications": false
            }
        }],
        "x-generic-subscription-framing": {
            "description": "Fixture framing.",
            "notifications": {
                "fixture/event": { "$ref": "schema.json#/$defs/FixtureEvent" }
            }
        }
    });

    let rendered = render(&document);

    assert!(rendered.contains("# Fixture Protocol API reference"));
    assert!(rendered.contains("### <code>fixture/custom</code>"));
    assert!(rendered.contains("| <code>by-name</code> | Subscription | Yes | No |"));
    assert!(rendered.contains(
        "| <code>payload</code> | Yes | <a href=\"../schema.json#/$defs/FixturePayload\""
    ));
    assert!(rendered.contains(
        "| <code>fixtureResult</code> | <code>{&quot;type&quot;:[&quot;string&quot;,&quot;null&quot;]}</code> |"
    ));
    assert!(
        rendered.contains(
            "| <code>fixture/event</code> | <a href=\"../schema.json#/$defs/FixtureEvent\""
        )
    );
    assert!(rendered.contains("<a href=\"../openrpc.json\" download>"));
    assert!(rendered.ends_with("</code> |\n"));
}
