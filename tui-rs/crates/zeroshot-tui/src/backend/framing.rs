const HEADER_DELIMITER: &[u8] = b"\r\n\r\n";
const MAX_HEADER_BYTES: usize = 8 * 1024;

pub const MAX_FRAME_SIZE: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    MissingContentLength,
    InvalidContentLength(String),
    InvalidHeader(String),
    FrameTooLarge(usize),
    HeaderTooLarge(usize),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::MissingContentLength => write!(f, "Missing Content-Length header"),
            FrameError::InvalidContentLength(value) => {
                write!(f, "Invalid Content-Length: {value}")
            }
            FrameError::InvalidHeader(value) => write!(f, "Invalid header: {value}"),
            FrameError::FrameTooLarge(size) => write!(f, "Frame too large: {size} bytes"),
            FrameError::HeaderTooLarge(size) => write!(f, "Header too large: {size} bytes"),
        }
    }
}

impl std::error::Error for FrameError {}

pub struct FrameEncoder;

impl FrameEncoder {
    pub fn encode(payload: &[u8]) -> Result<Vec<u8>, FrameError> {
        if payload.len() > MAX_FRAME_SIZE {
            return Err(FrameError::FrameTooLarge(payload.len()));
        }
        let mut framed = Vec::with_capacity(payload.len() + 64);
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        framed.extend_from_slice(header.as_bytes());
        framed.extend_from_slice(payload);
        Ok(framed)
    }
}

#[derive(Debug, Default)]
pub struct FrameDecoder {
    buffer: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, FrameError> {
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() > MAX_FRAME_SIZE + MAX_HEADER_BYTES {
            return Err(FrameError::FrameTooLarge(self.buffer.len()));
        }
        let mut frames = Vec::new();
        loop {
            let header_end = match find_header_end(&self.buffer) {
                Some(index) => index,
                None => break,
            };
            if header_end > MAX_HEADER_BYTES {
                return Err(FrameError::HeaderTooLarge(header_end));
            }
            let header_bytes = &self.buffer[..header_end];
            let header_str = std::str::from_utf8(header_bytes)
                .map_err(|err| FrameError::InvalidHeader(err.to_string()))?;
            let content_length = parse_content_length(header_str)?;
            if content_length > MAX_FRAME_SIZE {
                return Err(FrameError::FrameTooLarge(content_length));
            }
            let payload_start = header_end + HEADER_DELIMITER.len();
            let payload_end = payload_start + content_length;
            if self.buffer.len() < payload_end {
                break;
            }
            let payload = self.buffer[payload_start..payload_end].to_vec();
            self.buffer.drain(0..payload_end);
            frames.push(payload);
        }
        Ok(frames)
    }
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(HEADER_DELIMITER.len())
        .position(|window| window == HEADER_DELIMITER)
}

fn parse_content_length(header: &str) -> Result<usize, FrameError> {
    let mut content_length: Option<usize> = None;
    for line in header.split("\r\n") {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(2, ':');
        let name = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        if name.eq_ignore_ascii_case("content-length") {
            if value.is_empty() {
                return Err(FrameError::InvalidContentLength(value.to_string()));
            }
            let parsed = value
                .parse::<usize>()
                .map_err(|_| FrameError::InvalidContentLength(value.to_string()))?;
            content_length = Some(parsed);
        }
    }
    content_length.ok_or(FrameError::MissingContentLength)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_content_length_handles_case() {
        let header = "Content-Length: 10\r\nX-Other: abc";
        assert_eq!(parse_content_length(header).unwrap(), 10);
        let header_lower = "content-length: 5";
        assert_eq!(parse_content_length(header_lower).unwrap(), 5);
    }
}
