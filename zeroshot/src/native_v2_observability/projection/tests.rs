use openengine_cluster_protocol::{REDACTED_ASSISTANT_OUTPUT, MAX_LOG_MESSAGE_BYTES};
use openengine_cluster_testkit::assertions::AssertValue;

use super::*;

#[test]
fn public_output_preserves_text_with_visible_control_escapes() {
    let source = "first\n\tsecond\r\u{1b}[31m café";
    let expected = "first\\n\\tsecond\\r\\u{1b}[31m café";

    assert_eq!(bounded_attach_output(source).as_str(), expected);
    assert_eq!(
        log_record(SafeLogStream::Output, source)
            .assert_value()
            .message
            .as_str(),
        expected
    );
}

#[test]
fn escaped_log_text_is_bounded_while_attachment_keeps_its_existing_redaction() {
    let source = format!("head {} tail", "\u{1b}".repeat(3_000));

    assert_eq!(
        bounded_attach_output(&source).as_str(),
        REDACTED_ASSISTANT_OUTPUT
    );
    let record = log_record(SafeLogStream::Output, &source).assert_value();
    let message = record.message.as_str();
    assert!(message.starts_with("head "));
    assert!(message.ends_with(" tail"));
    assert!(message.len() <= MAX_LOG_MESSAGE_BYTES);
    assert!(!message.chars().any(char::is_control));
}
