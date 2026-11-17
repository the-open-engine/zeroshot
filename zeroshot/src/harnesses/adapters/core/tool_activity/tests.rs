use super::*;
use serde_json::json;

fn identity() -> ToolIdentity {
    ToolIdentity {
        name: "Bash".into(),
        id: Some("call-1".into()),
    }
}

#[test]
fn tool_activity_bounds_whole_entries_and_preserves_identity_and_utf8_head_tail() {
    for length in [0, 10, 8192, 80_000] {
        let payload = json!(format!("HEAD{}TAIL", "界é".repeat(length)));
        for entry in [
            ToolEntry::call("Claude", identity(), payload.clone()),
            ToolEntry::completed("Claude", identity(), "error".into(), payload.clone()),
        ] {
            let mut emissions = Vec::new();
            entry.emit(&ProviderText::new(&[]), &mut emissions);
            assert_eq!(emissions.len(), 1);
            let text = &emissions[0].text;
            assert!(text.len() <= TOOL_ENTRY_BYTES, "{}", text.len());
            for expected in ["Bash", "call-1", "HEAD", "TAIL"] {
                assert!(text.contains(expected));
            }
            assert_eq!(text.contains(TRUNCATED), length >= 8192);
        }
    }
}

#[test]
fn tool_activity_redacts_decoded_and_escaped_credentials_before_excerpting() {
    let secret = "token-\"\\\n界-secret";
    let publisher = ProviderText::new(&[secret.into()]);
    // Put credentials across both excerpt cuts and inside JSON strings and keys.
    for padding in [3500, 4020, 4070, 8000] {
        let text = format!(
            "{secret}{}{secret}{}{secret}",
            "x".repeat(padding),
            "y".repeat(padding)
        );
        for payload in [json!(text), json!({secret: [text]})] {
            let entry = ToolEntry::completed("Pi", identity(), "success".into(), payload);
            let rendered = entry.render(&publisher);
            assert!(rendered.len() <= TOOL_ENTRY_BYTES);
            assert!(rendered.contains("[REDACTED]"));
            assert!(!rendered.contains("token-"));
            assert!(!rendered.contains("-secret"));
        }
    }
    let mut identity = identity();
    identity.name = secret.into();
    identity.id = Some(secret.into());
    let rendered =
        ToolEntry::completed("Pi", identity, secret.into(), json!(secret)).render(&publisher);
    assert!(!rendered.contains("token-"));
}

#[test]
fn tool_activity_redacts_numeric_credentials_before_excerpting() {
    let secret = "8675309123";
    let publisher = ProviderText::new(&[secret.into()]);
    let number: Value = serde_json::from_str(secret).unwrap();
    for harness in ["Codex", "Claude", "Copilot", "Pi"] {
        for payload in [
            number.clone(),
            json!({"head": [number], "padding": "界".repeat(9000), "tail": number}),
        ] {
            for entry in [
                ToolEntry::call(harness, identity(), payload.clone()),
                ToolEntry::completed(harness, identity(), "success".into(), payload.clone()),
            ] {
                let text = entry.render(&publisher);
                assert!(
                    !text.contains(secret),
                    "{harness}: numeric credential exposed"
                );
                assert!(text.contains("[REDACTED]"));
                assert!(text.len() <= TOOL_ENTRY_BYTES);
            }
        }
    }
}

#[test]
fn tool_activity_large_identifiers_cannot_displace_other_fields() {
    let mut identity = identity();
    identity.name = "n".repeat(20_000);
    let rendered = ToolEntry::completed("Codex", identity, "failed exit=1".into(), json!("result"))
        .render(&ProviderText::new(&[]));
    assert!(rendered.len() <= TOOL_ENTRY_BYTES);
    assert!(rendered.contains("call-1"));
    assert!(rendered.contains("failed exit=1"));
    assert!(rendered.contains(TRUNCATED));
}

#[test]
fn tool_activity_deduplicates_each_phase_and_keeps_the_call_name() {
    let mut activity = ToolActivity::default();
    assert!(activity.record(identity(), false).is_some());
    assert!(activity.record(identity(), false).is_none());
    let completion = ToolIdentity {
        name: "unknown".into(),
        ..identity()
    };
    assert_eq!(
        activity.record(completion.clone(), true).unwrap().name,
        "Bash"
    );
    assert!(activity.record(completion, true).is_none());
    assert!(activity.record(identity(), false).is_none());
    assert_eq!(error_status(None), "unknown");
}

#[test]
fn tool_activity_bounds_control_character_expansion_in_cli_and_attach() {
    for text in ["\n", "\t", "\r", "\u{85}"] {
        let payload = json!(format!("HEAD{}TAIL", text.repeat(10_000)));
        let mut identity = identity();
        identity.name = format!("name{}end", text.repeat(1000));
        let rendered = ToolEntry::completed("Codex", identity, "failed".into(), payload)
            .render(&ProviderText::new(&[]));
        let visible = crate::native_v2_contract::visible_text(&rendered);
        assert!(visible.len() <= TOOL_ENTRY_BYTES, "{}", visible.len());
        for expected in ["HEAD", "TAIL", "call-1", "failed", "[truncated]"] {
            assert!(visible.contains(expected));
        }
    }
}
