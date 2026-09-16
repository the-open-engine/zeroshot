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
fn verifier_permission_policy_rejects_writes_escalation_and_unknown_tools() {
    assert!(permission_allowed(&json!({"kind":"read"}), false));
    assert!(!permission_allowed(&json!({"kind":"write"}), false));
    assert!(permission_allowed(&json!({"kind":"write"}), true));
    assert!(!permission_allowed(
        &json!({"kind":"write","managedApprovalRequired":true}),
        true
    ));
    assert!(!permission_allowed(
        &json!({"kind":"shell","requestSandboxBypass":true}),
        true
    ));
    assert!(!permission_allowed(&json!({"kind":"future"}), true));
    let shell = json!({"kind":"shell", "hasWriteFileRedirection":false,
        "commands":[{"readOnly":true}]});
    assert!(permission_allowed(&shell, false));
    assert!(!permission_allowed(&json!({"kind":"shell"}), false));
}
