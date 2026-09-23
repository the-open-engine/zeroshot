use openengine_cluster_protocol::{REDACTED_ASSISTANT_OUTPUT, REDACTED_LOG_MESSAGE};
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
fn public_output_redacts_when_visible_escapes_exceed_the_wire_bound() {
    let source = "\u{1b}".repeat(3_000);

    assert_eq!(
        bounded_attach_output(&source).as_str(),
        REDACTED_ASSISTANT_OUTPUT
    );
    assert_eq!(
        log_record(SafeLogStream::Output, &source)
            .assert_value()
            .message
            .as_str(),
        REDACTED_LOG_MESSAGE
    );
}
