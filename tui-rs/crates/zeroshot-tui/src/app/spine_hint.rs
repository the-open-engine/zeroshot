use crate::commands::{parse, VALID_PROVIDERS};
use crate::protocol::GuidanceDeliveryResult;

use super::{
    detect_issue_reference, resolve_focus_target, resolve_spine_agent_target,
    resolve_spine_cluster_target, AgentKey, AppState, SpineMode, ToastLevel, UiVariant,
    ZoomStackContext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpineHintTone {
    Muted,
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpineHint {
    pub text: String,
    pub tone: SpineHintTone,
}

impl SpineHint {
    pub fn new(text: impl Into<String>, tone: SpineHintTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }

    pub fn empty() -> Self {
        Self::new("", SpineHintTone::Muted)
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn from_toast(text: String, level: ToastLevel) -> Self {
        let tone = match level {
            ToastLevel::Info => SpineHintTone::Info,
            ToastLevel::Success => SpineHintTone::Success,
            ToastLevel::Error => SpineHintTone::Error,
        };
        Self::new(text, tone)
    }
}

impl Default for SpineHint {
    fn default() -> Self {
        Self::empty()
    }
}

pub fn compute_spine_hint(state: &AppState) -> SpineHint {
    match state.spine.mode {
        SpineMode::Command => command_hint(state),
        SpineMode::Intent => intent_hint(state),
        SpineMode::WhisperCluster => whisper_cluster_hint(state),
        SpineMode::WhisperAgent => whisper_agent_hint(state),
    }
}

fn command_hint(state: &AppState) -> SpineHint {
    let trimmed = state.spine.input.input.trim();
    if trimmed.is_empty() {
        return SpineHint::new(command_help_line(), SpineHintTone::Muted);
    }

    let raw = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };

    match parse(&raw) {
        Ok(parsed) => match parsed.name.as_str() {
            "help" => SpineHint::new(command_help_line(), SpineHintTone::Info),
            "monitor" => {
                let label = if matches!(state.ui_variant, UiVariant::Disruptive) {
                    "Fleet Radar"
                } else {
                    "Monitor"
                };
                SpineHint::new(format!("Open {label}"), SpineHintTone::Info)
            }
            "issue" => {
                let Some(reference) = parsed.args.first() else {
                    return SpineHint::new(issue_usage(), SpineHintTone::Error);
                };
                SpineHint::new(
                    format!("Start cluster from issue {reference}"),
                    SpineHintTone::Info,
                )
            }
            "guide" => guidance_command_hint(state, &parsed.args, "Guide", true),
            "nudge" => guidance_command_hint(state, &parsed.args, "Nudge", true),
            "interrupt" => guidance_command_hint(state, &parsed.args, "Interrupt", false),
            "pin" => pin_command_hint(state),
            "provider" => provider_hint(&parsed.args),
            "quit" | "exit" => SpineHint::new("Quit TUI", SpineHintTone::Info),
            other => SpineHint::new(
                format!("Unknown command: {other}. Try /help."),
                SpineHintTone::Error,
            ),
        },
        Err(err) => SpineHint::new(err.to_string(), SpineHintTone::Error),
    }
}

fn provider_hint(args: &[String]) -> SpineHint {
    let Some(name) = args.first() else {
        return SpineHint::new(provider_usage(), SpineHintTone::Error);
    };
    let normalized = name.to_lowercase();
    if !VALID_PROVIDERS.contains(&normalized.as_str()) {
        return SpineHint::new(
            format!(
                "Unknown provider '{name}'. Use one of: {}",
                VALID_PROVIDERS.join(", ")
            ),
            SpineHintTone::Error,
        );
    }
    SpineHint::new(
        format!("Set provider override to {normalized}"),
        SpineHintTone::Info,
    )
}

fn command_help_line() -> String {
    "Commands: /help /monitor /issue <ref> /provider <name> /guide <text> /nudge <text> /interrupt [text] /pin /quit /exit".to_string()
}

fn provider_usage() -> String {
    format!("Usage: /provider <{}>", VALID_PROVIDERS.join("|"))
}

fn issue_usage() -> &'static str {
    "Usage: /issue <ref>"
}

fn guidance_command_hint(
    state: &AppState,
    args: &[String],
    verb: &str,
    require_text: bool,
) -> SpineHint {
    if require_text && args.is_empty() {
        return SpineHint::new(
            format!("Usage: /{} <text>", verb.to_lowercase()),
            SpineHintTone::Error,
        );
    }
    let Some(target) = resolve_focus_target(state) else {
        return SpineHint::new(
            "Select a cluster or agent to guide.".to_string(),
            SpineHintTone::Error,
        );
    };
    let label = target.label();
    SpineHint::new(format!("{verb} {label}"), SpineHintTone::Info)
}

fn pin_command_hint(state: &AppState) -> SpineHint {
    if !matches!(state.ui_variant, UiVariant::Disruptive) {
        return SpineHint::new(
            "Pinning is only available in Disruptive UI.".to_string(),
            SpineHintTone::Error,
        );
    }
    let Some(target) = resolve_focus_target(state) else {
        return SpineHint::new(
            "Select a cluster or agent to pin.".to_string(),
            SpineHintTone::Error,
        );
    };
    let action = if state.pinned_target.as_ref() == Some(&target) {
        "Unpin"
    } else {
        "Pin"
    };
    SpineHint::new(format!("{action} {}", target.label()), SpineHintTone::Info)
}

fn intent_hint(state: &AppState) -> SpineHint {
    if !matches!(state.zoom_stack_context(), ZoomStackContext::Root) {
        return SpineHint::empty();
    }

    let trimmed = state.spine.input.input.trim();
    if trimmed.is_empty() {
        return SpineHint::empty();
    }

    let mut hint = if detect_issue_reference(trimmed).is_some() {
        SpineHint::new("Start cluster from issue", SpineHintTone::Info)
    } else {
        SpineHint::new("Start cluster from text", SpineHintTone::Info)
    };

    if let Some(provider) = state.provider_override.as_deref() {
        hint.text = format!("{} (provider: {provider})", hint.text);
    }

    hint
}

fn whisper_cluster_hint(state: &AppState) -> SpineHint {
    let Some(cluster_id) = resolve_spine_cluster_target(state) else {
        return SpineHint::empty();
    };
    SpineHint::new(
        format!("Whisper to cluster {cluster_id}"),
        SpineHintTone::Info,
    )
}

fn whisper_agent_hint(state: &AppState) -> SpineHint {
    let Some((cluster_id, agent_id)) = resolve_spine_agent_target(state) else {
        return SpineHint::empty();
    };
    let mut hint = SpineHint::new(
        format!("Whisper to agent {agent_id} @ {cluster_id}"),
        SpineHintTone::Info,
    );
    if let Some(status) = guidance_status_hint(state, &cluster_id, &agent_id) {
        hint.text = format!("{} ({status})", hint.text);
    }
    hint
}

fn guidance_status_hint(
    state: &AppState,
    cluster_id: &str,
    agent_id: &str,
) -> Option<&'static str> {
    let key = AgentKey::new(cluster_id, agent_id);
    let agent_state = state.agents.get(&key)?;
    let result = agent_state.last_guidance.as_ref()?;
    guidance_status(result)
}

fn guidance_status(result: &GuidanceDeliveryResult) -> Option<&'static str> {
    match result.status.to_lowercase().as_str() {
        "injected" => Some("likely injected"),
        "queued" => Some("likely queued"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{ScreenId, SpineAction};
    use crate::protocol::GuidanceDeliveryResult;
    use crate::screens::agent;

    #[test]
    fn command_provider_missing_arg_shows_usage() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::Command;
        state.spine.input.input = "provider".to_string();

        let hint = compute_spine_hint(&state);

        assert_eq!(hint.tone, SpineHintTone::Error);
        assert!(hint.text.contains("Usage: /provider"));
    }

    #[test]
    fn command_unknown_shows_error() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::Command;
        state.spine.input.input = "nope".to_string();

        let hint = compute_spine_hint(&state);

        assert_eq!(hint.tone, SpineHintTone::Error);
        assert!(hint.text.contains("Unknown command"));
    }

    #[test]
    fn intent_issue_prediction() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::Intent;
        state.spine.input.input = "123".to_string();

        let hint = compute_spine_hint(&state);

        assert!(hint.text.contains("Start cluster from issue"));
    }

    #[test]
    fn intent_text_prediction() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::Intent;
        state.spine.input.input = "Implement X".to_string();

        let hint = compute_spine_hint(&state);

        assert!(hint.text.contains("Start cluster from text"));
    }

    #[test]
    fn whisper_agent_includes_delivery_hint() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::WhisperAgent;
        state.screen_stack = vec![ScreenId::AgentMicroscope {
            cluster_id: "cluster-1".to_string(),
            agent_id: "agent-1".to_string(),
        }];
        let mut agent_state = agent::State::default();
        agent_state.last_guidance = Some(GuidanceDeliveryResult {
            status: "queued".to_string(),
            reason: None,
            method: Some("pty".to_string()),
            task_id: None,
        });
        state.agents.insert(
            AgentKey::new("cluster-1".to_string(), "agent-1".to_string()),
            agent_state,
        );

        let hint = compute_spine_hint(&state);

        assert!(hint.text.contains("agent-1"));
        assert!(hint.text.contains("cluster-1"));
        assert!(hint.text.contains("queued"));
    }

    #[test]
    fn spine_action_insert_char_updates_hint_without_backend_effects() {
        let mut state = AppState::default();
        state.spine.mode = SpineMode::Intent;

        let (next, effects) = crate::app::update(
            state,
            crate::app::Action::Spine(SpineAction::InsertChar('1')),
        );

        assert!(effects.is_empty());
        assert!(next.spine.hint.text.contains("Start cluster from issue"));
    }
}
