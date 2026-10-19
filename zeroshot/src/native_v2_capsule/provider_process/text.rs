//! Safe, bounded publication of provider text across native event boundaries.
use crate::native_v2_runner::LiveOutputStream;
use super::{ProviderEmission, safe_provider_text};

pub(crate) const LIVE_CHUNK_BYTES: usize = 8 * 1024;
pub(crate) const LIVE_LINE_BYTES: usize = 1024;

#[derive(Default)]
struct PendingText {
    text: String,
    /// Stream and end offset in `text` of each part, in order.
    parts: Vec<(LiveOutputStream, usize)>,
    /// Whether the last part continues with the next text of its stream.
    open: bool,
}

impl PendingText {
    fn push(&mut self, stream: LiveOutputStream, text: &str) {
        if text.is_empty() {
            return;
        }
        self.text.push_str(text);
        match self.parts.last_mut() {
            Some((last, end)) if self.open && *last == stream => *end = self.text.len(),
            _ => self.parts.push((stream, self.text.len())),
        }
        self.open = true;
    }

    /// Start offset of the part containing the byte at `offset`.
    fn part_start(&self, offset: usize) -> usize {
        self.parts
            .iter()
            .map(|&(_, end)| end)
            .take_while(|&end| end <= offset)
            .last()
            .unwrap_or(0)
    }

    /// Drops the first `cut` bytes once they are published.
    fn consume(&mut self, cut: usize) {
        self.text.drain(..cut);
        self.parts.retain_mut(|(_, end)| {
            let remains = *end > cut;
            *end = end.saturating_sub(cut);
            remains
        });
    }
}

/// Batches streaming text while retaining any unfinished known credential across events.
pub(crate) struct ProviderText {
    redactions: Vec<String>,
    pending: PendingText,
}

impl ProviderText {
    pub(crate) fn new(redactions: &[String]) -> Self {
        Self {
            redactions: super::redaction_values(redactions.iter().map(String::as_str)),
            pending: PendingText::default(),
        }
    }

    pub(crate) fn redact(&self, text: &str) -> String {
        safe_provider_text(text, &self.redactions)
    }

    /// Failure-only preview; successful response text is never a diagnostic input.
    pub(crate) fn failure_diagnostic<'a>(
        &self,
        provider: &str,
        details: impl IntoIterator<Item = &'a str>,
    ) -> Option<String> {
        let detail = details
            .into_iter()
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>()
            .join("; ");
        (!detail.is_empty()).then(|| {
            super::provider_failure_diagnostic(provider, Some(&detail), None, &self.redactions)
        })
    }

    #[cfg(test)]
    pub(crate) fn buffered(&self) -> (&str, usize) {
        (&self.pending.text, self.pending.parts.len())
    }

    pub(crate) fn push(
        &mut self,
        emissions: &mut Vec<ProviderEmission>,
        stream: LiveOutputStream,
        text: &str,
    ) {
        if self
            .pending
            .parts
            .last()
            .is_some_and(|&(pending, _)| pending != stream)
        {
            self.boundary(emissions);
        }
        self.pending.push(stream, text);
        let cut = publishable_prefix(&self.pending.text, &self.redactions);
        self.publish_pending(emissions, cut);
    }

    /// Ends the current part at a block, message, record or stream boundary and publishes pending
    /// text. Redaction applies per published part, so when a part ends with text that could still
    /// begin a redacted value, its last line stays pending until later text completes or rules out
    /// that value, or the turn ends.
    pub(crate) fn boundary(&mut self, emissions: &mut Vec<ProviderEmission>) {
        let text = &self.pending.text;
        let unfinished = unfinished_redaction_start(text, &self.redactions);
        let line_start = text[..unfinished].rfind('\n').map_or(0, |index| index + 1);
        let held = line_start.max(self.pending.part_start(unfinished));
        let cut = redaction_boundary(text, &self.redactions, held);
        self.publish_pending(emissions, cut);
        self.pending.open = false;
    }

    /// Publishes all pending text once the turn's output ends.
    pub(crate) fn finish(&mut self, emissions: &mut Vec<ProviderEmission>) {
        self.publish_pending(emissions, self.pending.text.len());
    }

    /// Publishes pending text up to `cut`, which no redacted value crosses, each part as its own
    /// records. A redacted value that crosses parts is published with the part where it ends.
    fn publish_pending(&mut self, emissions: &mut Vec<ProviderEmission>, cut: usize) {
        let mut start = 0;
        for &(stream, end) in &self.pending.parts {
            let end = redaction_boundary(&self.pending.text, &self.redactions, end.min(cut));
            if start < end {
                self.emit(emissions, stream, &self.pending.text[start..end]);
                start = end;
            }
        }
        self.pending.consume(cut);
    }

    pub(crate) fn emit(
        &self,
        emissions: &mut Vec<ProviderEmission>,
        stream: LiveOutputStream,
        text: &str,
    ) {
        let safe = self.redact(text);
        for chunk in utf8_chunks(&safe, LIVE_CHUNK_BYTES) {
            emissions.push(ProviderEmission {
                stream,
                text: chunk,
                diagnostic: None,
            });
        }
    }
}

/// Byte length of the leading part of `text` to publish now: whole lines, or the text up to a word
/// break once it exceeds `LIVE_LINE_BYTES`. Redaction applies per published part, so the tail that
/// could still begin a redacted value stays pending, and no part ends inside one.
fn publishable_prefix(text: &str, redactions: &[String]) -> usize {
    let holdback = redactions
        .iter()
        .map(|value| value.len().saturating_sub(1))
        .max()
        .unwrap_or(0);
    let mut limit = text.len().saturating_sub(holdback);
    while !text.is_char_boundary(limit) {
        limit -= 1;
    }
    let candidate = &text[..limit];
    let cut = match candidate.rfind('\n') {
        Some(index) => index + 1,
        None if limit >= LIVE_LINE_BYTES => candidate
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map_or(limit, |(index, character)| index + character.len_utf8()),
        None => 0,
    };
    redaction_boundary(text, redactions, cut)
}

/// Moves `cut` back to the start of any redacted value in `text` that it would split.
fn redaction_boundary(text: &str, redactions: &[String], mut cut: usize) -> usize {
    while let Some(start) = redactions.iter().find_map(|value| {
        text.match_indices(value.as_str())
            .map(|(start, _)| start)
            .find(|&start| start < cut && cut < start + value.len())
    }) {
        cut = start;
    }
    cut
}

/// Byte offset of the longest tail of `text` that could still begin a redacted value, or the
/// length of `text` when none could.
fn unfinished_redaction_start(text: &str, redactions: &[String]) -> usize {
    redactions
        .iter()
        .filter_map(|value| {
            let longest = value.len().saturating_sub(1).min(text.len());
            (text.len() - longest..text.len())
                .find(|&start| text.is_char_boundary(start) && value.starts_with(&text[start..]))
        })
        .min()
        .unwrap_or(text.len())
}

pub(crate) fn utf8_chunks(value: &str, max: usize) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < value.len() {
        let mut end = value.len().min(start + max);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(value[start..end].to_owned());
        start = end;
    }
    chunks
}
