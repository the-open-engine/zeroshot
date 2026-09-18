use super::*;
use crate::native_v2_copilot::{framing::Frames, events::permission_allowed};

#[test]
fn framing_handles_fragmented_consecutive_messages_and_rejects_truncation() {
    let mut frames = Frames::default();
    for byte in b"Content-Length: 2\r\n\r\n{}Content-Length: 4\r\n\r\nnull" {
        frames.push(&[*byte]).assert_value();
    }
    assert_eq!(frames.pop(), Some(json!({})));
    assert_eq!(frames.pop(), Some(Value::Null));
    frames.finish().assert_value();
    frames.push(b"Content-Length: 12\r\n\r\n{").assert_value();
    assert!(frames.finish().is_err());
}

#[test]
fn framing_rejects_oversized_and_ambiguous_records() {
    for invalid in [
        "Content-Length: 67108865\r\n\r\n",
        "Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}",
        "Content-Length: nope\r\n\r\n",
        "Other: 2\r\n\r\n{}",
    ] {
        assert!(Frames::default().push(invalid.as_bytes()).is_err());
    }
}

#[test]
fn agent_permissions_allow_checks_and_reject_escalation_and_unknown_tools() {
    for kind in ["read", "url", "write", "shell"] {
        assert!(permission_allowed(&json!({"kind":kind})));
        assert!(!permission_allowed(
            &json!({"kind":kind,"managedApprovalRequired":true})
        ));
        assert!(!permission_allowed(
            &json!({"kind":kind,"requestSandboxBypass":true})
        ));
    }
    assert!(!permission_allowed(&json!({"kind":"future"})));
    assert!(!permission_allowed(&json!({})));
    assert!(permission_allowed(
        &json!({"kind":"shell", "hasWriteFileRedirection":true,
        "commands":[{"readOnly":false}]})
    ));
}
