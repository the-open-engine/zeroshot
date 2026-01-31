use std::str;

use super::{BackendError, MAX_FRAME_SIZE};

#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, BackendError> {
        if !bytes.is_empty() {
            self.buffer.extend_from_slice(bytes);
        }

        let mut frames = Vec::new();
        loop {
            let header_end = match find_header_end(&self.buffer) {
                Some(idx) => idx,
                None => break,
            };

            let header_bytes = &self.buffer[..header_end];
            let content_length = parse_content_length(header_bytes)?;
            if content_length > MAX_FRAME_SIZE {
                return Err(BackendError::Frame(format!(
                    "content length {content_length} exceeds max {MAX_FRAME_SIZE}"
                )));
            }

            let payload_start = header_end + 4;
            let payload_end = payload_start + content_length;
            if self.buffer.len() < payload_end {
                break;
            }

            let payload = self.buffer[payload_start..payload_end].to_vec();
            self.buffer.drain(..payload_end);
            frames.push(payload);
        }

        Ok(frames)
    }
}

pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, BackendError> {
    if payload.len() > MAX_FRAME_SIZE {
        return Err(BackendError::Frame(format!(
            "payload size {size} exceeds max {MAX_FRAME_SIZE}",
            size = payload.len()
        )));
    }

    let mut out = Vec::with_capacity(payload.len() + 64);
    out.extend_from_slice(format!("Content-Length: {}\r\n\r\n", payload.len()).as_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
}

fn parse_content_length(header_bytes: &[u8]) -> Result<usize, BackendError> {
    let header_str = str::from_utf8(header_bytes)
        .map_err(|_| BackendError::Frame("header is not valid utf-8".to_string()))?;

    let mut content_length = None;
    for line in header_str.split("\r\n") {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, ':');
        let name = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        if name.eq_ignore_ascii_case("content-length") {
            let parsed: usize = value.parse().map_err(|_| {
                BackendError::Frame(format!("invalid Content-Length value: {value}"))
            })?;
            content_length = Some(parsed);
        }
    }

    content_length.ok_or_else(|| BackendError::Frame("missing Content-Length".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trip_single() {
        let payload = br#"{"jsonrpc":"2.0","method":"ping"}"#;
        let encoded = encode_frame(payload).expect("encode");

        let mut decoder = FrameDecoder::new();
        let frames = decoder.push_bytes(&encoded).expect("decode");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], payload);
    }

    #[test]
    fn frame_round_trip_split() {
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
    fn frame_round_trip_multiple() {
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
    fn frame_rejects_oversize() {
        let header = format!("Content-Length: {}\r\n\r\n", MAX_FRAME_SIZE + 1);
        let mut decoder = FrameDecoder::new();
        let err = decoder
            .push_bytes(header.as_bytes())
            .expect_err("should error");
        assert!(matches!(err, BackendError::Frame(_)));
    }
}
