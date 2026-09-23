use super::*;

#[test]
fn unavailable_history_is_an_error_even_before_the_first_event() {
    for mut cursor in [None, Some(Cursor::new("v2:3"))] {
        let before = cursor.clone();
        let mut output = Vec::new();
        let result = write_durable_event(
            DurableItem::Closed(SubscriptionCloseReason::SourceUnavailable),
            &mut cursor,
            &mut output,
        );
        assert!(
            matches!(result, Err(NativeV2CliError::Protocol(message)) if message.contains("incomplete"))
        );
        assert_eq!(cursor, before);
        assert!(output.is_empty());
    }
}
