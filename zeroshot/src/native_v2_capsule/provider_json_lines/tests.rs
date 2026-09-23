use super::*;

#[test]
fn frames_split_crlf_blank_and_unterminated_records() {
    let mut lines = ProviderJsonLines::with_max_record_bytes(64);
    assert!(lines.push(b"{\"a\":").is_empty());
    assert_eq!(
        lines.push(b"1}\r\n\nlast"),
        [
            ProviderJsonLine::Record(br#"{"a":1}"#.to_vec()),
            ProviderJsonLine::Record(Vec::new()),
        ]
    );
    assert_eq!(
        lines.finish(),
        Some(ProviderJsonLine::Record(b"last".to_vec()))
    );
    assert_eq!(lines.finish(), None);
}

#[test]
fn discards_only_the_oversized_record_and_recovers() {
    let mut lines = ProviderJsonLines::with_max_record_bytes(4);
    assert_eq!(
        lines.push(b"1234\n12345\nok\n"),
        [
            ProviderJsonLine::Record(b"1234".to_vec()),
            ProviderJsonLine::Oversized,
            ProviderJsonLine::Record(b"ok".to_vec()),
        ]
    );
}

#[test]
fn discards_an_oversized_record_across_chunks_and_at_eof() {
    let mut lines = ProviderJsonLines::with_max_record_bytes(4);
    assert!(lines.push(b"123").is_empty());
    assert!(lines.push(b"45").is_empty());
    assert_eq!(lines.finish(), Some(ProviderJsonLine::Oversized));

    assert!(lines.push(b"good").is_empty());
    assert_eq!(
        lines.finish(),
        Some(ProviderJsonLine::Record(b"good".to_vec()))
    );
}

#[test]
fn releases_the_pending_allocation_when_a_record_is_discarded() {
    let mut lines = ProviderJsonLines::with_max_record_bytes(4);
    assert!(lines.push(b"1234").is_empty());
    assert!(lines.pending.capacity() >= 4);
    assert!(lines.push(b"5").is_empty());
    assert_eq!(lines.pending.capacity(), 0);
    lines.discard();
    assert_eq!(lines.pending.capacity(), 0);
}
