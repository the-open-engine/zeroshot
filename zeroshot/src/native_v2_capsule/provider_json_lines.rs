//! Streaming framing for provider-owned JSON Lines output.
//!
//! Provider turns may legitimately produce an unbounded number of records. This helper therefore
//! bounds only one unfinished record, which is the memory it retains between process chunks.

pub(crate) const MAX_PROVIDER_JSONL_RECORD_BYTES: usize = 64 * 1024 * 1024;

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
