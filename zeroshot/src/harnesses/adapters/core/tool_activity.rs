//! Bounded text records for native tool activity; this is not a public event schema.
use std::collections::BTreeMap;

use serde_json::Value;

use crate::native_v2_capsule::provider_process::{ProviderEmission, text::ProviderText};
use crate::native_v2_runner::LiveOutputStream;

pub(crate) const TOOL_ENTRY_BYTES: usize = 8 * 1024;
const TRUNCATED: &str = "\n...[truncated]...\n";

#[derive(Clone)]
pub(crate) struct ToolIdentity {
    pub name: String,
    pub id: Option<String>,
}

/// Native IDs identify a call throughout repeated snapshots, including after completion.
#[derive(Default)]
pub(crate) struct ToolActivity {
    seen: BTreeMap<String, (String, bool, bool)>,
}

impl ToolActivity {
    pub(crate) fn record(
        &mut self,
        mut identity: ToolIdentity,
        completed: bool,
    ) -> Option<ToolIdentity> {
        if let Some(id) = &identity.id {
            let (name, call_seen, completion_seen) = self
                .seen
                .entry(id.clone())
                .or_insert_with(|| (identity.name.clone(), false, false));
            if identity.name == "unknown" {
                identity.name.clone_from(name);
            } else if name == "unknown" {
                name.clone_from(&identity.name);
            }
            let seen = if completed {
                completion_seen
            } else {
                call_seen
            };
            if *seen {
                return None;
            }
            *seen = true;
        }
        Some(identity)
    }
}

pub(crate) struct ToolEntry {
    harness: &'static str,
    identity: ToolIdentity,
    phase: &'static str,
    status: String,
    payload: Value,
}

impl ToolEntry {
    pub(crate) fn call(harness: &'static str, identity: ToolIdentity, input: Value) -> Self {
        Self {
            harness,
            identity,
            phase: "call",
            status: "unknown".into(),
            payload: input,
        }
    }

    pub(crate) fn completed(
        harness: &'static str,
        identity: ToolIdentity,
        status: String,
        result: Value,
    ) -> Self {
        Self {
            harness,
            identity,
            phase: "completed",
            status,
            payload: result,
        }
    }

    pub(crate) fn render(&self, publisher: &ProviderText) -> String {
        // Bound each identifying field separately so a huge name cannot displace ID/status.
        let field = |text: &str| excerpt(&publisher.redact(text), 256);
        let header = format!(
            "{} tool {}: {} id={} status={}\n",
            self.harness,
            self.phase,
            field(&self.identity.name),
            field(self.identity.id.as_deref().unwrap_or("unknown")),
            field(&self.status)
        );
        // Redact decoded strings (including object keys) before JSON re-escaping. Redact the
        // serialization too, since numbers and literals can contain known credentials.
        let safe = redact_value(&self.payload, publisher);
        let payload = match safe {
            Value::String(text) => text,
            value => publisher.redact(&value.to_string()),
        };
        format!(
            "{header}{}",
            excerpt(&payload, TOOL_ENTRY_BYTES - rendered_bytes(&header))
        )
    }

    pub(crate) fn emit(&self, publisher: &ProviderText, emissions: &mut Vec<ProviderEmission>) {
        emissions.push(ProviderEmission {
            stream: LiveOutputStream::System,
            text: self.render(publisher),
            diagnostic: None,
        });
    }
}

pub(crate) fn error_status(is_error: Option<bool>) -> String {
    match is_error {
        Some(true) => "error",
        Some(false) => "success",
        None => "unknown",
    }
    .into()
}

fn redact_value(value: &Value, publisher: &ProviderText) -> Value {
    match value {
        Value::String(text) => Value::String(publisher.redact(text)),
        Value::Array(values) => values
            .iter()
            .map(|value| redact_value(value, publisher))
            .collect(),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| (publisher.redact(key), redact_value(value, publisher)))
            .collect(),
        value => value.clone(),
    }
}

// CLI/attach render controls with `visible_text`; account for that expansion while retaining
// the original line breaks in the ledger and node history.
fn character_bytes(character: char) -> usize {
    if character.is_control() {
        character.escape_default().count()
    } else {
        character.len_utf8()
    }
}

fn rendered_bytes(text: &str) -> usize {
    text.chars().map(character_bytes).sum()
}

fn excerpt(text: &str, limit: usize) -> String {
    if rendered_bytes(text) <= limit {
        return text.to_owned();
    }
    let remaining = limit - rendered_bytes(TRUNCATED);
    let mut head = 0;
    let mut used = 0;
    for character in text.chars() {
        let size = character_bytes(character);
        if used + size > remaining.div_ceil(2) {
            break;
        }
        used += size;
        head += character.len_utf8();
    }
    let mut tail = text.len();
    used = 0;
    for character in text.chars().rev() {
        let size = character_bytes(character);
        if used + size > remaining / 2 {
            break;
        }
        used += size;
        tail -= character.len_utf8();
    }
    format!("{}{TRUNCATED}{}", &text[..head], &text[tail..])
}

#[cfg(test)]
mod tests;
