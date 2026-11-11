use super::*;

#[test]
fn invalid_unicode_in_the_last_result_does_not_restore_an_earlier_answer() {
    for bad in [br"\uD83D".as_slice(), br"\uDE00", b"\xff", b"\xf0\x9f"] {
        for ending in [b"".as_slice(), b"\r\n"] {
            let mut bytes = Vec::new();
            append_event(&mut bytes, success(json!("earlier"), "session-1"));
            bytes.extend_from_slice(
                br#"{"type":"result","subtype":"success","session_id":"session-1","structured_output":{"response":"final:"#,
            );
            bytes.extend_from_slice(bad);
            bytes.extend_from_slice(br#""}}"#);
            bytes.extend_from_slice(ending);
            for chunk_bytes in [1, 7, bytes.len()] {
                let mut transcript = ClaudeTranscript::new(Vec::new());
                for chunk in bytes.chunks(chunk_bytes) {
                    transcript.push(chunk);
                }
                transcript.finish_stream();
                assert_eq!(transcript.malformed_records, 0);
                assert_eq!(
                    completed_response(transcript.finish(None).assert_value()),
                    "final:�"
                );
            }
        }
    }
}

#[test]
fn decoded_unicode_is_redacted_from_live_text() {
    for bad in [br"\uD83D".as_slice(), b"\xff"] {
        let mut bytes = br#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"\u0074oken-"#.to_vec();
        bytes.extend_from_slice(bad);
        bytes.extend_from_slice(br#"-end"}}}"#);
        let mut transcript = ClaudeTranscript::new(vec!["token-�-end".to_owned()]);
        let mut emissions = Vec::new();
        for chunk in bytes.chunks(1) {
            emissions.extend(transcript.push(chunk));
        }
        emissions.extend(transcript.finish_stream());
        assert_eq!(
            emitted(&emissions),
            [(LiveOutputStream::Output, "[REDACTED]")]
        );
    }
}

#[test]
fn malformed_json_with_a_surrogate_is_counted_and_skipped() {
    let mut transcript = ClaudeTranscript::new(Vec::new());
    transcript.push(
        br#"{"type":"result","subtype":"success","result":"\uD83D",}
{"type":"result","subtype":"success","result":"\uD83D\u12"}
"#,
    );
    let mut bytes = Vec::new();
    append_event(&mut bytes, success(json!("valid"), "session-1"));
    transcript.push(&bytes);
    assert_eq!(transcript.malformed_records, 2);
    assert_eq!(
        completed_response(transcript.finish(None).assert_value()),
        "valid"
    );
}
