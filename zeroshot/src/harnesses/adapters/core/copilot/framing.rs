use std::collections::VecDeque;
use serde_json::Value;
use crate::native_v2_runner::NodeRunnerError;
use super::rpc::failure;

const MAX_HEADER: usize = 16 * 1024;
pub(super) const MAX_BODY: usize = 64 * 1024 * 1024;

/// Content-Length framing used by Copilot's stdio RPC. Only an unfinished message is retained.
#[derive(Default)]
pub(super) struct Frames {
    pending: Vec<u8>,
    length: Option<usize>,
    ready: VecDeque<Value>,
}

impl Frames {
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<(), NodeRunnerError> {
        self.pending.extend_from_slice(bytes);
        loop {
            if self.length.is_none() && !self.header()? {
                return Ok(());
            }
            let length = self.length.ok_or(NodeRunnerError::Driver)?;
            if self.pending.len() < length {
                return Ok(());
            }
            let body = self.pending.drain(..length).collect::<Vec<_>>();
            self.ready.push_back(
                serde_json::from_slice(&body)
                    .map_err(|_| failure("Copilot sent invalid JSON-RPC"))?,
            );
            self.length = None;
            if self.pending.is_empty() {
                return Ok(());
            }
        }
    }

    fn header(&mut self) -> Result<bool, NodeRunnerError> {
        let end = self
            .pending
            .windows(4)
            .position(|window| window == b"\r\n\r\n");
        let Some(end) = end else {
            if self.pending.len() > MAX_HEADER {
                return Err(failure("Copilot RPC header is oversized"));
            }
            return Ok(false);
        };
        if end > MAX_HEADER {
            return Err(failure("Copilot RPC header is oversized"));
        }
        let header = std::str::from_utf8(&self.pending[..end])
            .map_err(|_| failure("Copilot RPC header is invalid"))?;
        let length = parse_length(header)?;
        self.pending.drain(..end + 4);
        self.length = Some(length);
        Ok(true)
    }

    pub(super) fn pop(&mut self) -> Option<Value> {
        self.ready.pop_front()
    }

    pub(super) fn finish(&self) -> Result<(), NodeRunnerError> {
        if !self.pending.is_empty() || self.length.is_some() {
            return Err(failure("Copilot RPC stream ended inside a message"));
        }
        Ok(())
    }
}

fn parse_length(header: &str) -> Result<usize, NodeRunnerError> {
    let mut length = None;
    for line in header.split("\r\n") {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| failure("Copilot RPC header is invalid"))?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(failure("Copilot RPC length is duplicated"));
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| failure("Copilot RPC length is invalid"))?,
            );
        }
    }
    let length = length
        .filter(|value| *value > 0 && *value <= MAX_BODY)
        .ok_or_else(|| failure("Copilot RPC length is missing or oversized"))?;
    Ok(length)
}
