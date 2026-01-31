use zeroshot_tui::backend::framing::{FrameDecoder, FrameEncoder, FrameError, MAX_FRAME_SIZE};

#[test]
fn single_frame_roundtrip() {
    let payload = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let framed = FrameEncoder::encode(payload).expect("encode");
    let mut decoder = FrameDecoder::new();
    let frames = decoder.push(&framed).expect("decode");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0], payload);
}

#[test]
fn multiple_frames_in_one_buffer() {
    let payload_a = br#"{"jsonrpc":"2.0","id":1,"method":"one"}"#;
    let payload_b = br#"{"jsonrpc":"2.0","id":2,"method":"two"}"#;
    let mut combined = Vec::new();
    combined.extend(FrameEncoder::encode(payload_a).unwrap());
    combined.extend(FrameEncoder::encode(payload_b).unwrap());

    let mut decoder = FrameDecoder::new();
    let frames = decoder.push(&combined).expect("decode");
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0], payload_a);
    assert_eq!(frames[1], payload_b);
}

#[test]
fn split_frame_across_chunks() {
    let payload = br#"{"jsonrpc":"2.0","id":3,"method":"split"}"#;
    let framed = FrameEncoder::encode(payload).unwrap();
    let mid = framed.len() / 2;

    let mut decoder = FrameDecoder::new();
    let frames = decoder.push(&framed[..mid]).expect("decode");
    assert!(frames.is_empty());

    let frames = decoder.push(&framed[mid..]).expect("decode");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0], payload);
}

#[test]
fn oversized_frame_rejected() {
    let oversized = MAX_FRAME_SIZE + 1;
    let header = format!("Content-Length: {}\r\n\r\n", oversized);
    let mut decoder = FrameDecoder::new();
    let error = decoder.push(header.as_bytes()).expect_err("oversized");
    assert!(matches!(error, FrameError::FrameTooLarge(_)));
}

#[test]
fn encoder_rejects_oversized_payload() {
    let payload = vec![0u8; MAX_FRAME_SIZE + 1];
    let error = FrameEncoder::encode(&payload).expect_err("oversized");
    assert!(matches!(error, FrameError::FrameTooLarge(_)));
}
