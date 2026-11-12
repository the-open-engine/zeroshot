//! Stable native diagnostic markers, never wording from the provider message.
use serde_json::Value;

pub(super) fn unrecognized_model(stderr: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stderr).lines().find_map(|line| {
        let payload = line.strip_prefix("[claude-code:unrecognized_model] ")?;
        let value: Value = serde_json::from_str(payload).ok()?;
        value
            .get("model")?
            .as_str()
            .filter(|model| !model.is_empty())
            .map(str::to_owned)
    })
}
