use serde_json::{Value, json};
use openengine_cluster_protocol::TokenCount;
use crate::native_v2_contract::TokenUsageDelta;
use crate::native_v2_capsule::provider_process::safe_provider_text;
use crate::native_v2_runner::{LiveOutput, LiveOutputStream, NodeRunnerError};
use super::rpc::{CopilotRpc, failure};

pub(super) async fn receive(
    rpc: &mut CopilotRpc<'_>,
    params: &Value,
) -> Result<(), NodeRunnerError> {
    let event = &params["event"];
    let kind = event["type"].as_str().unwrap_or("");
    if !known_event(kind) {
        return Ok(());
    }
    require_session(rpc, params)?;
    let data = &event["data"];
    match kind {
        "assistant.usage" => usage(rpc, data).await,
        "assistant.message" => assistant_message(rpc, data).await,
        "session.error" => session_error(rpc, data).await,
        "tool.execution_start" => tool_started(rpc, data).await,
        "permission.requested" => permission(rpc, data),
        "tool.execution_complete" => tool_completed(rpc, data).await,
        _ => Ok(()),
    }
}

async fn session_error(rpc: &mut CopilotRpc<'_>, data: &Value) -> Result<(), NodeRunnerError> {
    let message = data["message"]
        .as_str()
        .ok_or_else(|| failure("Copilot session error is invalid"))?;
    rpc.provider_error = Some(safe_provider_text(message, &rpc.redactions));
    emit(rpc, LiveOutputStream::Error, message).await
}

async fn tool_started(rpc: &CopilotRpc<'_>, data: &Value) -> Result<(), NodeRunnerError> {
    let name = data["toolName"]
        .as_str()
        .ok_or_else(|| failure("Copilot tool name is invalid"))?;
    emit(
        rpc,
        LiveOutputStream::System,
        &format!("Copilot tool: {name}"),
    )
    .await
}

async fn tool_completed(rpc: &CopilotRpc<'_>, data: &Value) -> Result<(), NodeRunnerError> {
    if data["success"].as_bool() == Some(false) {
        let detail = data["error"]["message"]
            .as_str()
            .unwrap_or("unknown tool failure");
        emit(
            rpc,
            LiveOutputStream::Error,
            &format!("Copilot tool failed: {detail}"),
        )
        .await?;
    }
    Ok(())
}

fn known_event(kind: &str) -> bool {
    matches!(
        kind,
        "assistant.message"
            | "assistant.usage"
            | "session.error"
            | "permission.requested"
            | "tool.execution_start"
            | "tool.execution_complete"
    )
}

async fn assistant_message(rpc: &mut CopilotRpc<'_>, data: &Value) -> Result<(), NodeRunnerError> {
    if data
        .get("parentToolCallId")
        .is_some_and(|value| !value.is_null())
    {
        return Ok(());
    }
    let content = data["content"]
        .as_str()
        .ok_or_else(|| failure("Copilot assistant content is invalid"))?;
    emit(rpc, LiveOutputStream::Output, content).await?;
    rpc.response = Some(content.to_owned());
    Ok(())
}

pub(super) async fn receive_usage(
    rpc: &CopilotRpc<'_>,
    params: &Value,
) -> Result<(), NodeRunnerError> {
    if params["event"]["type"].as_str() == Some("assistant.usage") {
        require_session(rpc, params)?;
        usage(rpc, &params["event"]["data"]).await?;
    }
    Ok(())
}

fn require_session(rpc: &CopilotRpc<'_>, params: &Value) -> Result<(), NodeRunnerError> {
    if params["sessionId"].as_str() != Some(rpc.session_id.as_str()) {
        return Err(failure("Copilot event session identity is unexpected"));
    }
    Ok(())
}

async fn usage(rpc: &CopilotRpc<'_>, data: &Value) -> Result<(), NodeRunnerError> {
    let usage = TokenUsageDelta {
        input_tokens: count(data, "inputTokens")?,
        output_tokens: count(data, "outputTokens")?,
        cache_read_input_tokens: optional_count(data, "cacheReadTokens")?,
        cache_creation_input_tokens: optional_count(data, "cacheWriteTokens")?,
    };
    rpc.control.record_token_usage(Some(usage)).await
}

fn count(data: &Value, name: &str) -> Result<TokenCount, NodeRunnerError> {
    data[name]
        .as_u64()
        .and_then(|value| TokenCount::new(value).ok())
        .ok_or_else(|| failure("Copilot token usage is invalid or out of range"))
}

fn optional_count(data: &Value, name: &str) -> Result<Option<TokenCount>, NodeRunnerError> {
    if data.get(name).is_none_or(Value::is_null) {
        return Ok(None);
    }
    count(data, name).map(Some)
}

async fn emit(
    rpc: &CopilotRpc<'_>,
    stream: LiveOutputStream,
    text: &str,
) -> Result<(), NodeRunnerError> {
    let safe = safe_provider_text(text, &rpc.redactions);
    let mut remaining = safe.as_str();
    while !remaining.is_empty() {
        let mut end = remaining.len().min(8 * 1024);
        while !remaining.is_char_boundary(end) {
            end -= 1;
        }
        let (part, rest) = remaining.split_at(end);
        rpc.control.emit(LiveOutput::new(stream, part)?).await?;
        remaining = rest;
    }
    Ok(())
}

fn permission(rpc: &mut CopilotRpc<'_>, data: &Value) -> Result<(), NodeRunnerError> {
    if data["resolvedByHook"].as_bool() == Some(true) {
        return Ok(());
    }
    let request_id = data["requestId"]
        .as_str()
        .filter(|id| !id.is_empty())
        .ok_or_else(|| failure("Copilot permission request identity is invalid"))?;
    let request = &data["permissionRequest"];
    let allowed = permission_allowed(request);
    rpc.send_request(
        "session.permissions.handlePendingPermissionRequest",
        json!({
            "sessionId":rpc.session_id, "requestId":request_id,
            "result":{"kind":if allowed {"approve-once"} else {"reject"}},
        }),
    )?;
    Ok(())
}

pub(super) fn permission_allowed(request: &Value) -> bool {
    if request["managedApprovalRequired"].as_bool() == Some(true)
        || request["requestSandboxBypass"].as_bool() == Some(true)
    {
        return false;
    }
    matches!(
        request["kind"].as_str(),
        Some("read" | "url" | "write" | "shell")
    )
}
