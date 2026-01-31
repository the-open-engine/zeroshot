use zeroshot_tui::backend::{encode_frame, FrameDecoder, MAX_FRAME_SIZE};

#[test]
fn framing_single_round_trip() {
    let payload = br#"{"jsonrpc":"2.0","method":"ping"}"#;
    let encoded = encode_frame(payload).expect("encode");

    let mut decoder = FrameDecoder::new();
    let frames = decoder.push_bytes(&encoded).expect("decode");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0], payload);
}

#[test]
fn framing_multiple_frames_in_one_read() {
    let payload_a = br#"{"jsonrpc":"2.0","method":"a"}"#;
    let payload_b = br#"{"jsonrpc":"2.0","method":"b"}"#;

    let mut encoded = encode_frame(payload_a).expect("encode a");
    encoded.extend_from_slice(&encode_frame(payload_b).expect("encode b"));

    let mut decoder = FrameDecoder::new();
    let frames = decoder.push_bytes(&encoded).expect("decode");
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0], payload_a);
    assert_eq!(frames[1], payload_b);
}

#[test]
fn framing_split_frame_across_reads() {
    let payload = br#"{"jsonrpc":"2.0","method":"ping"}"#;
    let encoded = encode_frame(payload).expect("encode");
    let split = encoded.len() / 2;

    let mut decoder = FrameDecoder::new();
    let frames = decoder.push_bytes(&encoded[..split]).expect("decode");
    assert!(frames.is_empty());

    let frames = decoder.push_bytes(&encoded[split..]).expect("decode");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0], payload);
}

#[test]
fn framing_rejects_oversize_frames() {
    let header = format!("Content-Length: {}\r\n\r\n", MAX_FRAME_SIZE + 1);
    let mut decoder = FrameDecoder::new();
    let err = decoder
        .push_bytes(header.as_bytes())
        .expect_err("should reject oversize");
    assert!(format!("{err}").contains("exceeds"));
}
