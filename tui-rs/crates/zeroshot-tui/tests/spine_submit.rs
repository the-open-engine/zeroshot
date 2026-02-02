use zeroshot_tui::app::{
    self, Action, AppState, BackendRequest, Effect, ScreenId, SpineAction, SpineMode,
};

#[test]
fn intent_submit_text_starts_cluster_from_text() {
    let mut state = AppState::default();
    state.spine.mode = SpineMode::Intent;
    state.spine.input.input = "build".to_string();
    state.spine.input.cursor = 5;

    let (state, effects) = app::update(state, Action::Spine(SpineAction::Submit));

    assert!(
        effects.contains(&Effect::Backend(BackendRequest::StartClusterFromText {
            text: "build".to_string(),
            provider_override: None,
        }))
    );
    assert_eq!(state.spine.input.input, "");
    assert_eq!(state.spine.mode, SpineMode::Intent);
}

#[test]
fn intent_submit_issue_starts_cluster_from_issue() {
    let mut state = AppState::default();
    state.spine.mode = SpineMode::Intent;
    state.spine.input.input = "org/repo#42".to_string();
    state.spine.input.cursor = 11;

    let (state, effects) = app::update(state, Action::Spine(SpineAction::Submit));

    assert!(
        effects.contains(&Effect::Backend(BackendRequest::StartClusterFromIssue {
            reference: "org/repo#42".to_string(),
            provider_override: None,
        }))
    );
    assert_eq!(state.spine.input.input, "");
    assert_eq!(state.spine.mode, SpineMode::Intent);
}

#[test]
fn whisper_cluster_submit_sends_guidance_to_cluster() {
    let mut state = AppState::default();
    state.screen_stack = vec![ScreenId::ClusterCanvas {
        id: "cluster-1".to_string(),
    }];
    state.spine.mode = SpineMode::WhisperCluster;
    state.spine.input.input = "ping".to_string();
    state.spine.input.cursor = 4;

    let (state, effects) = app::update(state, Action::Spine(SpineAction::Submit));

    assert!(
        effects.contains(&Effect::Backend(BackendRequest::SendGuidanceToCluster {
            cluster_id: "cluster-1".to_string(),
            message: "ping".to_string(),
        }))
    );
    assert_eq!(state.spine.input.input, "");
    assert_eq!(state.spine.mode, SpineMode::Intent);
}

#[test]
fn whisper_agent_submit_sends_guidance_to_agent() {
    let mut state = AppState::default();
    state.screen_stack = vec![ScreenId::AgentMicroscope {
        cluster_id: "cluster-1".to_string(),
        agent_id: "agent-1".to_string(),
    }];
    state.spine.mode = SpineMode::WhisperAgent;
    state.spine.input.input = "ping".to_string();
    state.spine.input.cursor = 4;

    let (state, effects) = app::update(state, Action::Spine(SpineAction::Submit));

    assert!(
        effects.contains(&Effect::Backend(BackendRequest::SendGuidanceToAgent {
            cluster_id: "cluster-1".to_string(),
            agent_id: "agent-1".to_string(),
            message: "ping".to_string(),
        }))
    );
    assert_eq!(state.spine.input.input, "");
    assert_eq!(state.spine.mode, SpineMode::WhisperAgent);
}
