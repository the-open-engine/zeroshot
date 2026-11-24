//! Wire translation for the physical run identities used by cluster/v2.
//! Only method-owned fields are visited. Graphs, payloads, request answers and logs are opaque.

use serde_json::Value;

pub const VERSION: &str = "openengine.cluster/v2";

pub fn run_method(method: &str) -> bool {
    method.starts_with("run/") || method.starts_with("inbox/")
}

fn rename(value: &mut Value, from: &str, to: &str) {
    if let Some(object) = value.as_object_mut() {
        if let Some(field) = object.remove(from) {
            object.insert(to.to_owned(), field);
        }
    }
}

fn copy_run(value: &mut Value, key: &str) {
    if let Some(object) = value.as_object_mut() {
        if let Some(id) = object.get("runId").cloned() {
            object.entry(key).or_insert(id);
        }
    }
}

/// Physical clients address the run they named, even after another part has started.
pub fn read_request(method: &str, params: &mut Value) -> bool {
    if params.get("partId").is_some() || params.get("successorPartId").is_some() {
        return false;
    }
    if matches!(
        method,
        crate::RUN_STATUS_METHOD
            | crate::RUN_WATCH_METHOD
            | crate::RUN_LOGS_METHOD
            | crate::RUN_ATTACH_METHOD
            | crate::RUN_VECTOR_METHOD
            | crate::RUN_VECTORS_METHOD
            | crate::RUN_CHECKPOINTS_METHOD
            | crate::INBOX_SHOW_METHOD
            | crate::RUN_RESUME_METHOD
    ) {
        copy_run(params, "partId");
    }
    if method == crate::RUN_RESUME_METHOD {
        rename(params, "successorRunId", "successorPartId");
    }
    true
}

/// A physical server cannot prove membership of a different part in a logical run.
pub fn write_request(params: &mut Value) -> bool {
    if let Some(object) = params.as_object_mut() {
        if let Some(id) = object.remove("partId") {
            if object.get("runId") != Some(&id) {
                return false;
            }
        }
    }
    rename(params, "successorPartId", "successorRunId");
    true
}

/// Supply physical identities when reading an unmodified v2 response into current types.
pub fn read_response(method: &str, value: &mut Value) {
    match method {
        crate::RUN_SUBMIT_METHOD => copy_run(value, "currentPartId"),
        crate::RUN_RESUME_METHOD => {
            copy_run(value, "currentPartId");
            rename(value, "resumedFrom", "resumedFromPartId");
        }
        crate::RUN_LIST_METHOD => entries(value, "runs", |v| {
            read_response(crate::RUN_STATUS_METHOD, v)
        }),
        crate::RUN_STATUS_METHOD | crate::RUN_FORCE_METHOD | crate::RUN_SUSPEND_METHOD => {
            copy_run(value, "currentPartId");
            copy_run(value, "partId");
        }
        crate::RUN_VECTORS_METHOD => {
            copy_run(value, "partId");
            entries(value, "vectors", |v| copy_run(v, "partId"));
        }
        crate::INBOX_LIST_METHOD => entries(value, "requests", |v| copy_run(v, "partId")),
        crate::RUN_ATTACH_METHOD
        | crate::RUN_VECTOR_METHOD
        | crate::RUN_CHECKPOINTS_METHOD
        | crate::RUN_DISCARD_WORKSPACE_METHOD
        | crate::RUN_STEER_METHOD
        | crate::INBOX_SHOW_METHOD
        | "event" => copy_run(value, "partId"),
        _ => {}
    }
}

/// Strip logical identity fields before a strict v2 peer receives the response or event.
pub fn write_response(method: &str, value: &mut Value) {
    match method {
        crate::RUN_LIST_METHOD => entries(value, "runs", |v| {
            write_response(crate::RUN_STATUS_METHOD, v)
        }),
        crate::RUN_VECTORS_METHOD => entries(value, "vectors", |v| {
            write_response(crate::RUN_VECTOR_METHOD, v)
        }),
        crate::INBOX_LIST_METHOD => entries(value, "requests", |v| {
            write_response(crate::INBOX_SHOW_METHOD, v)
        }),
        crate::RUN_RESUME_METHOD => rename(value, "resumedFromPartId", "resumedFrom"),
        crate::INBOX_SHOW_METHOD => rename(value, "successorPartId", "successorRunId"),
        crate::RUN_VECTOR_METHOD
            if value.get("kind").and_then(Value::as_str) == Some("continuation") =>
        {
            if let Some(object) = value.as_object_mut() {
                object.remove("checkpointId");
            }
        }
        _ => {}
    }
    physical_identity(method, value);
}

fn physical_identity(method: &str, value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        // Submit/resume have no partId. A status inspected on an ancestor does.
        let current = object
            .remove("currentPartId")
            .filter(|_| method != crate::RUN_SUBMIT_METHOD);
        if let Some(id) = object.remove("partId").or(current) {
            object.insert("runId".to_owned(), id);
        }
    }
}

fn entries(value: &mut Value, field: &str, mut visit: impl FnMut(&mut Value)) {
    if let Some(items) = value.get_mut(field).and_then(Value::as_array_mut) {
        items.iter_mut().for_each(&mut visit);
    }
}
