use super::*;

#[test]
fn agent_messages_accept_invalid_unicode_across_chunks() {
    for bad in [br"\uD83D".as_slice(), br"\uDE00", b"\xff", b"\xf0\x9f"] {
        for ending in [b"".as_slice(), b"\r\n"] {
            let mut bytes =
                br#"{"type":"item.completed","item":{"type":"agent_message","text":"answer:"#
                    .to_vec();
            bytes.extend_from_slice(bad);
            bytes.extend_from_slice(b"\"}}\n{\"type\":\"turn.completed\"}");
            bytes.extend_from_slice(ending);
            for chunk_bytes in [1, 7, bytes.len()] {
                let mut decoder = CodexOutputDecoder::new();
                let mut emissions = Vec::new();
                for chunk in bytes.chunks(chunk_bytes) {
                    emissions.extend(decoder.push(chunk));
                }
                let messages: Vec<_> = emissions
                    .iter()
                    .filter_map(|emission| match emission {
                        CodexEmission::AgentMessage(text) => Some(text.as_str()),
                        CodexEmission::Progress(_) => None,
                    })
                    .collect();
                assert_eq!(messages, ["answer:�"]);
                let output = decoder.finish();
                assert_eq!(output.malformed_records, 0);
                assert_eq!(output.final_message().assert_value(), "answer:�");
                assert!(output.failure.is_none());
            }
        }
    }
}

#[test]
fn malformed_json_with_a_surrogate_is_counted_and_skipped() {
    let output = CodexOutput::parse(
        br#"{"type":"item.completed","item":{"type":"agent_message","text":"\uD83D",}}
{"type":"item.completed","item":{"type":"agent_message","text":"\uD83D\u12"}}
{"type":"item.completed","item":{"type":"agent_message","text":"valid"}}
{"type":"turn.completed"}
"#,
    );
    assert_eq!(output.malformed_records, 2);
    assert_eq!(output.final_message().assert_value(), "valid");
    assert!(output.failure.is_none());
}
