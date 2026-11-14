//! Decode the unpaged inventory with a bound per status, including its full request.

use openengine_cluster_protocol::{MAX_REQUEST_RESPONSE_BYTES, bounded_json_from_slice};
use reqwest::Response;
use zeroshot_engine::native_v2_cli::{CliRunListResult, CliRunStatusResult};

use super::{TargetAuthorityError, authority_error};
use super::super::contract::MAX_RESPONSE_BYTES;

pub(super) async fn read_run_list(
    mut response: Response,
) -> Result<CliRunListResult, TargetAuthorityError> {
    let mut reader = RunListReader::default();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        TargetAuthorityError::request_failed("hosted run list response read", &error)
    })? {
        for byte in chunk {
            reader.push(byte)?;
        }
    }
    reader.finish()
}

#[derive(Default)]
enum Position {
    #[default]
    Prefix,
    First,
    Next,
    Separator,
    Done,
}

#[derive(Default)]
struct RunListReader {
    position: Position,
    // The small outer object is validated separately with an empty runs array.
    framing: Vec<u8>,
    item: Vec<u8>,
    depth: usize,
    quoted: bool,
    escaped: bool,
    runs: Vec<CliRunStatusResult>,
}

impl RunListReader {
    fn push(&mut self, byte: u8) -> Result<(), TargetAuthorityError> {
        if self.depth != 0 {
            return self.push_item(byte);
        }
        if matches!(self.position, Position::Prefix | Position::Done) {
            self.framing.push(byte);
            if self.framing.len() > MAX_RESPONSE_BYTES {
                return Err(too_large());
            }
            if matches!(self.position, Position::Prefix) && !self.in_string(byte) && byte == b'[' {
                self.framing.extend_from_slice(b"]}");
                bounded_json_from_slice::<CliRunListResult>(&self.framing)
                    .map_err(|_| malformed())?;
                self.framing.truncate(self.framing.len() - 2);
                self.position = Position::First;
            }
            return Ok(());
        }
        if matches!(byte, b' ' | b'\n' | b'\r' | b'\t') {
            return Ok(());
        }
        match (&self.position, byte) {
            (Position::First | Position::Next, b'{') => {
                self.item.push(byte);
                self.depth = 1;
            }
            (Position::First | Position::Separator, b']') => {
                self.framing.push(byte);
                self.position = Position::Done;
            }
            (Position::Separator, b',') => self.position = Position::Next,
            _ => return Err(malformed()),
        }
        Ok(())
    }

    fn push_item(&mut self, byte: u8) -> Result<(), TargetAuthorityError> {
        self.item.push(byte);
        if self.item.len() > MAX_REQUEST_RESPONSE_BYTES {
            return Err(too_large());
        }
        if !self.in_string(byte) {
            match byte {
                b'{' | b'[' => self.depth += 1,
                b'}' | b']' => self.depth -= 1,
                _ => {}
            }
        }
        if self.depth == 0 {
            self.runs
                .push(bounded_json_from_slice(&self.item).map_err(|_| malformed())?);
            self.item.clear();
            self.position = Position::Separator;
        }
        Ok(())
    }

    // Only locates object boundaries; serde validates every complete object and the envelope.
    fn in_string(&mut self, byte: u8) -> bool {
        if self.escaped {
            self.escaped = false;
        } else if self.quoted && byte == b'\\' {
            self.escaped = true;
        } else if byte == b'"' {
            self.quoted = !self.quoted;
            return true;
        }
        self.quoted
    }

    fn finish(self) -> Result<CliRunListResult, TargetAuthorityError> {
        if !matches!(self.position, Position::Done) || self.depth != 0 {
            return Err(malformed());
        }
        bounded_json_from_slice::<CliRunListResult>(&self.framing).map_err(|_| malformed())?;
        Ok(CliRunListResult { runs: self.runs })
    }
}

fn malformed() -> TargetAuthorityError {
    authority_error("hosted run list response is malformed")
}

fn too_large() -> TargetAuthorityError {
    authority_error("hosted run list response entry is too large")
}

#[cfg(test)]
#[path = "list_http/tests.rs"]
mod tests;
