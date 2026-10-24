use serde_json::{Value, json};

pub(super) const MAX_INPUT_BYTES: usize = 256 * 1024;
const BASE_BYTES: usize = 128 * 1024;

/// Count the actual serialized string carried by NodeInvocation, including its JSON escaping.
pub(super) fn encoded_size(value: &Value) -> usize {
    serde_json::to_string(value)
        .ok()
        .and_then(|text| serde_json::to_vec(&text).ok())
        .map_or(usize::MAX, |bytes| bytes.len())
}

pub(super) fn base(mut value: Value) -> Value {
    while encoded_size(&value) > BASE_BYTES {
        value["evidence"]["evidence_truncated"] = json!(true);
        let evidence = &mut value["evidence"];
        if shrink_largest_array(evidence)
            || shrink_string(evidence, "authored_graph")
            || shrink_string(evidence, "initial_input")
        {
            continue;
        }
        value["evidence"] = json!({"evidence_truncated":true,"omitted_reason":"serialized evidence exceeds the postmortem context budget"});
        break;
    }
    value
}

pub(super) fn add_array(document: &mut Value, key: &str, item: Value) -> bool {
    let Some(items) = document.get_mut(key).and_then(Value::as_array_mut) else {
        return false;
    };
    items.push(item);
    if encoded_size(document) <= MAX_INPUT_BYTES - 4096 {
        return true;
    }
    if let Some(items) = document.get_mut(key).and_then(Value::as_array_mut) {
        items.pop();
    }
    false
}

pub(super) fn add_object(document: &mut Value, key: &str, id: &str, item: Value) -> bool {
    let Some(items) = document.get_mut(key).and_then(Value::as_object_mut) else {
        return false;
    };
    items.insert(id.to_owned(), item);
    if encoded_size(document) <= MAX_INPUT_BYTES - 4096 {
        return true;
    }
    if let Some(items) = document.get_mut(key).and_then(Value::as_object_mut) {
        items.remove(id);
    }
    false
}

fn shrink_largest_array(document: &mut Value) -> bool {
    let mut sizes = ["records", "executions", "tool_observations"]
        .map(|key| (encoded_size(&document[key]), key));
    sizes.sort_by_key(|item| std::cmp::Reverse(item.0));
    sizes
        .into_iter()
        .any(|(_, key)| shrink_array(document, key))
}

fn shrink_records(items: &mut Vec<Value>) -> bool {
    let mut counts = std::collections::BTreeMap::<u64, usize>::new();
    for record in items.iter() {
        let Some(execution) = record.get("execution").and_then(Value::as_u64) else {
            return false;
        };
        *counts.entry(execution).or_default() += 1;
    }
    let mut remove = counts
        .into_iter()
        .map(|(execution, count)| (execution, count / 2))
        .collect::<std::collections::BTreeMap<_, _>>();
    let before = items.len();
    items.retain(|record| {
        let remaining = record
            .get("execution")
            .and_then(Value::as_u64)
            .and_then(|execution| remove.get_mut(&execution));
        if let Some(remaining) = remaining
            && *remaining > 0
        {
            *remaining -= 1;
            false
        } else {
            true
        }
    });
    before != items.len()
}

fn shrink_array(document: &mut Value, key: &str) -> bool {
    let Some(items) = document.get_mut(key).and_then(Value::as_array_mut) else {
        return false;
    };
    if items.is_empty() {
        return false;
    }
    if key == "records"
        && items
            .iter()
            .all(|record| record.get("execution").and_then(Value::as_u64).is_some())
    {
        return shrink_records(items);
    }
    if key == "tool_observations" {
        let quarter = items.len() / 4;
        if quarter == 0 {
            items.clear();
        } else {
            items.drain(quarter..items.len() - quarter);
        }
    } else if key == "records" {
        items.drain(..items.len().div_ceil(2));
    } else {
        items.truncate(items.len() / 2);
    }
    true
}

fn shrink_string(document: &mut Value, key: &str) -> bool {
    let Some(text) = document.get_mut(key).and_then(|value| value.as_str()) else {
        return false;
    };
    if text.len() < 256 {
        return false;
    }
    let text = crate::native_v2_runner::bounded_log_text(text.to_owned(), text.len() / 2);
    document[key] = json!(text);
    true
}

#[cfg(test)]
#[path = "context_budget/tests.rs"]
mod tests;
