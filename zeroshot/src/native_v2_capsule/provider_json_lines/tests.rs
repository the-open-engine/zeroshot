use super::*;

#[test]
fn replaces_only_invalid_unicode_in_provider_json() {
    for (bytes, expected) in [
        (br#""\uD83D""#.as_slice(), "�"),
        (br#""\ud83d!\ude00""#, "�!�"),
        (br#""\uD83D\uD83D\uDE00\uDE00""#, "�😀�"),
        (br#""\uD83D\u0041""#, "�A"),
        (br#""\u0000\u20ac\uD83D\uDE00""#, "\0€😀"),
        (br#""\\uD83D\\uDE00\uD83D""#, r"\uD83D\uDE00�"),
        (br#""\\\uD83D""#, "\\�"),
        (b"\"\xff\"", "�"),
        (b"\"\xf0\x9f\"", "�"),
        (b"\"\xff\\uD83D\"", "��"),
        ("\"😀\"".as_bytes(), "😀"),
    ] {
        assert_eq!(parse_provider_json(bytes).unwrap(), expected, "{bytes:?}");
    }
    assert_eq!(
        parse_provider_json(br#"{"\uD83D":["\uDE00"]}"#).unwrap(),
        serde_json::json!({"�":["�"]})
    );
}

#[test]
fn unicode_replacement_does_not_repair_other_malformed_json() {
    for bytes in [
        br#"{"text":"\uD83D",}"#.as_slice(),
        br#"{"text":"\uD83D""#,
        br#""\uD83D\u12""#,
        br#""\uD83D\uXYZ0""#,
        br#""\uD83D\x20""#,
        br#""\uD83D" trailing"#,
        br#""\uD83D" "another value""#,
        br#"{"text":"\uD83D","value":NaN}"#,
        br#"\uD83D"#,
        b"{\"text\":\"\xff\\uD83D\n\"}",
        b"{\"text\":\xff}",
    ] {
        assert!(parse_provider_json(bytes).is_err(), "{bytes:?}");
    }
}

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
