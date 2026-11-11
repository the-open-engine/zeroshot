use super::*;

#[test]
fn final_assistant_messages_accept_invalid_unicode_across_chunks() {
    for bad in [br"\uD83D".as_slice(), br"\uDE00", b"\xff", b"\xf0\x9f"] {
        for ending in [b"".as_slice(), b"\r\n"] {
            let mut bytes = format!("{}\n", assistant("earlier")).into_bytes();
            bytes.extend_from_slice(br#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"answer:"#);
            bytes.extend_from_slice(bad);
            bytes.extend_from_slice(br#""}],"stopReason":"stop"}}"#);
            bytes.extend_from_slice(ending);
            for chunk_bytes in [1, 7, bytes.len()] {
                let mut transcript = PiTranscript::new(Vec::new());
                for chunk in bytes.chunks(chunk_bytes) {
                    transcript.push(chunk);
                }
                let super::super::PiAttempt::Complete(result) = transcript.finish(None).unwrap()
                else {
                    panic!("expected a complete attempt");
                };
                assert_eq!(result.message, "answer:�");
            }
        }
    }
}

#[test]
fn decoded_unicode_is_redacted_from_live_text() {
    for bad in [br"\uD83D".as_slice(), b"\xff"] {
        let mut bytes = br#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","delta":"\u0074oken-"#.to_vec();
        bytes.extend_from_slice(bad);
        bytes.extend_from_slice(br#"-end"}}"#);
        let mut transcript = PiTranscript::new(vec!["token-�-end".to_owned()]);
        let mut emissions = Vec::new();
        for chunk in bytes.chunks(1) {
            emissions.extend(transcript.push(chunk));
        }
        emissions.extend(transcript.finish_stream());
        let text: String = emissions
            .iter()
            .map(|emission| emission.text.as_str())
            .collect();
        assert_eq!(text, "[REDACTED]");
    }
}

#[test]
fn malformed_json_with_a_surrogate_is_skipped() {
    let mut transcript = PiTranscript::new(Vec::new());
    transcript.push(
        br#"{"type":"message_end","message":{"role":"assistant","content":"\uD83D",}}
{"type":"message_end","message":{"role":"assistant","content":"\uD83D\u12"}}
"#,
    );
    assert!(!transcript.is_success());
    transcript.push(assistant("valid").to_string().as_bytes());
    let super::super::PiAttempt::Complete(result) = transcript.finish(None).unwrap() else {
        panic!("expected a complete attempt");
    };
    assert_eq!(result.message, "valid");
}
