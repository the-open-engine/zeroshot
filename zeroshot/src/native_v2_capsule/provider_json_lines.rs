//! Streaming framing for provider-owned JSON Lines output.
//!
//! Provider turns may legitimately produce an unbounded number of records. This helper therefore
//! bounds only one unfinished record, which is the memory it retains between process chunks.

use serde_json::Value;

pub(crate) const MAX_PROVIDER_JSONL_RECORD_BYTES: usize = 64 * 1024 * 1024;

/// Decode provider text lossily without accepting any other malformed JSON. Callers redact the
/// decoded strings, so escaped credentials receive the same treatment as literal text.
pub(crate) fn parse_provider_json(bytes: &[u8]) -> serde_json::Result<Value> {
    let text = String::from_utf8_lossy(bytes);
    match serde_json::from_str(&text) {
        Ok(value) => Ok(value),
        Err(error) => match replace_unpaired_surrogates(text.as_bytes()) {
            Some(bytes) => serde_json::from_slice(&bytes),
            None => Err(error),
        },
    }
}

fn replace_unpaired_surrogates(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut replaced = None;
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] != b'\\' {
            offset += 1;
            continue;
        }
        let Some(unit) = unicode_escape(&bytes[offset..]) else {
            // Skip the escaped character, especially a literal backslash before `uD83D`.
            offset += 2;
            continue;
        };
        if (0xD800..=0xDBFF).contains(&unit)
            && unicode_escape(&bytes[offset + 6..])
                .is_some_and(|next| (0xDC00..=0xDFFF).contains(&next))
        {
            offset += 12;
            continue;
        }
        if (0xD800..=0xDFFF).contains(&unit) {
            let output = replaced.get_or_insert_with(|| bytes.to_vec());
            output[offset..offset + 6].copy_from_slice(br"\uFFFD");
        }
        offset += 6;
    }
    replaced
}

fn unicode_escape(bytes: &[u8]) -> Option<u16> {
    let escape = bytes.get(..6)?;
    if !escape.starts_with(br"\u") {
        return None;
    }
    u16::from_str_radix(std::str::from_utf8(&escape[2..]).ok()?, 16).ok()
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ProviderJsonLine {
    Record(Vec<u8>),
    Oversized,
}

pub(crate) struct ProviderJsonLines {
    pending: Vec<u8>,
    discarding_oversized: bool,
    max_record_bytes: usize,
}

impl ProviderJsonLines {
    pub(crate) fn new() -> Self {
        Self::with_max_record_bytes(MAX_PROVIDER_JSONL_RECORD_BYTES)
    }

    fn with_max_record_bytes(max_record_bytes: usize) -> Self {
        Self {
            pending: Vec::new(),
            discarding_oversized: false,
            max_record_bytes,
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) -> Vec<ProviderJsonLine> {
        let mut records = Vec::new();
        let mut start = 0;
        for (end, byte) in bytes.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            self.append(bytes.get(start..end).unwrap_or_default());
            records.push(self.complete_record());
            start = end + 1;
        }
        self.append(bytes.get(start..).unwrap_or_default());
        records
    }

    pub(crate) fn finish(&mut self) -> Option<ProviderJsonLine> {
        if self.pending.is_empty() && !self.discarding_oversized {
            return None;
        }
        Some(self.complete_record())
    }

    pub(crate) fn discard(&mut self) {
        self.pending = Vec::new();
        self.discarding_oversized = false;
    }

    fn append(&mut self, bytes: &[u8]) {
        if self.discarding_oversized {
            return;
        }
        if bytes.len() <= self.max_record_bytes.saturating_sub(self.pending.len()) {
            self.pending.extend_from_slice(bytes);
            return;
        }
        self.pending = Vec::new();
        self.discarding_oversized = true;
    }

    fn complete_record(&mut self) -> ProviderJsonLine {
        if self.discarding_oversized {
            self.discarding_oversized = false;
            return ProviderJsonLine::Oversized;
        }
        let mut record = std::mem::take(&mut self.pending);
        if record.last() == Some(&b'\r') {
            record.pop();
        }
        ProviderJsonLine::Record(record)
    }
}

#[cfg(test)]
#[path = "provider_json_lines/tests.rs"]
mod tests;
