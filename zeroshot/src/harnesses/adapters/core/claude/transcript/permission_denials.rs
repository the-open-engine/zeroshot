use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

const LISTED_TOOLS: usize = 16;

/// Each result lists the calls its turn was denied as `{tool_name, tool_use_id, tool_input}`.
/// Denials are diagnostics only: they never change the attempt, and `tool_input` is never read.
#[derive(Default)]
pub(super) struct PermissionDenials {
    reported: BTreeSet<String>,
}

impl PermissionDenials {
    /// Unredacted, unbounded summary of the denied calls no earlier result of this process listed,
    /// by tool name and count, most frequent first.
    pub(super) fn summarize(&mut self, denials: Option<&Value>) -> Option<String> {
        let mut tools = BTreeMap::<&str, usize>::new();
        for denial in denials?.as_array()?.iter().filter_map(Value::as_object) {
            let id = denial.get("tool_use_id").and_then(Value::as_str);
            if id.is_some_and(|id| !self.reported.insert(id.to_owned())) {
                continue;
            }
            let tool = denial
                .get("tool_name")
                .and_then(Value::as_str)
                .unwrap_or("unnamed tool");
            *tools.entry(tool).or_default() += 1;
        }
        let calls = tools.values().sum::<usize>();
        if calls == 0 {
            return None;
        }
        let mut tools = tools.into_iter().collect::<Vec<_>>();
        tools.sort_by_key(|&(_, count)| Reverse(count));
        let mut listed = tools
            .iter()
            .take(LISTED_TOOLS)
            .map(|(tool, count)| format!("{tool} ({count})"))
            .collect::<Vec<_>>();
        let unlisted = tools.len().saturating_sub(LISTED_TOOLS);
        if unlisted > 0 {
            listed.push(format!("{unlisted} more tool{}", plural(unlisted)));
        }
        Some(format!(
            "Claude Code denied permission for {calls} tool call{}: {}. Denied calls do not change the node's outcome.",
            plural(calls),
            listed.join(", ")
        ))
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}
