//! NDJSON replies bound one status at a time in the unpaged run inventory.

use std::io;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use openengine_cluster_protocol::{
    MAX_REQUEST_RESPONSE_BYTES, RunStatusResult, bounded_json_from_slice,
};

pub(super) async fn next_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    let mut bounds = Bounds::default();
    loop {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                finish(line).map(Some)
            };
        }
        let end = chunk.iter().position(|byte| *byte == b'\n');
        let count = end.unwrap_or(chunk.len());
        let start = line.len();
        line.extend_from_slice(&chunk[..count]);
        for index in start..line.len() {
            bounds.push(&line[..=index])?;
        }
        reader.consume(count + usize::from(end.is_some()));
        if end.is_some() {
            return finish(line).map(Some);
        }
    }
}

fn finish(mut bytes: Vec<u8>) -> io::Result<String> {
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[derive(Default)]
struct Bounds {
    depth: usize,
    quoted: bool,
    escaped: bool,
    string_start: usize,
    last_string: (usize, usize),
    result: bool,
    result_key: bool,
    runs_key: bool,
    collection: bool,
    seen_collection: bool,
    framing: usize,
    segment: usize,
    item: Option<usize>,
}

impl Bounds {
    fn push(&mut self, prefix: &[u8]) -> io::Result<()> {
        let index = prefix.len() - 1;
        let byte = prefix[index];
        let measured = if !self.seen_collection {
            prefix.len()
        } else if self.collection {
            prefix.len() - self.segment
        } else {
            self.framing + prefix.len() - self.segment
        };
        if measured > super::MAX_FRAME_BYTES
            && !(byte == b'\r' && measured == super::MAX_FRAME_BYTES + 1)
        {
            return Err(too_large());
        }
        if self.escaped {
            self.escaped = false;
            return Ok(());
        }
        if self.quoted && byte == b'\\' {
            self.escaped = true;
            return Ok(());
        }
        if byte == b'"' {
            self.quoted = !self.quoted;
            if self.quoted {
                self.string_start = index;
            } else {
                self.last_string = (self.string_start, index + 1);
            }
            return Ok(());
        }
        if self.quoted {
            return Ok(());
        }
        self.structure(prefix)
    }

    // Locate collection and item boundaries only; typed decoding validates complete items.
    fn structure(&mut self, prefix: &[u8]) -> io::Result<()> {
        let index = prefix.len() - 1;
        let byte = prefix[index];
        match byte {
            b':' if self.depth <= 2 => {
                let (start, end) = self.last_string;
                let key = if end - start <= 38 {
                    serde_json::from_slice::<String>(&prefix[start..end]).ok()
                } else {
                    None
                };
                if self.depth == 1 {
                    self.result_key = key.as_deref() == Some("result");
                }
                if self.depth == 2 {
                    self.runs_key = key.as_deref() == Some("runs");
                }
            }
            b'{' => {
                if self.depth == 1 {
                    self.result = self.result_key;
                }
                if self.collection && self.depth == 3 {
                    self.item = Some(index);
                }
                self.depth += 1;
            }
            b'[' => {
                if self.depth == 2 && self.result && self.runs_key && !self.seen_collection {
                    self.collection = true;
                    self.seen_collection = true;
                    self.framing = prefix.len();
                    self.segment = prefix.len();
                }
                self.depth += 1;
            }
            b'}' | b']' => {
                self.depth = self.depth.saturating_sub(1);
                if self.collection && self.depth == 3 && byte == b'}' {
                    let start = self.item.take().ok_or_else(too_large)?;
                    let item = &prefix[start..];
                    if item.len() > MAX_REQUEST_RESPONSE_BYTES {
                        return Err(too_large());
                    }
                    bounded_json_from_slice::<RunStatusResult>(item)
                        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
                    self.segment = prefix.len();
                }
                if self.collection && self.depth == 2 && byte == b']' {
                    self.collection = false;
                    self.segment = index;
                }
                if self.depth == 1 {
                    self.result = false;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn too_large() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "response frame or run list entry is too large",
    )
}

#[cfg(test)]
mod tests;
