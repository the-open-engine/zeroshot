//! Claude Code's stream-json stdin: the prompt, then steers as further user messages.
//!
//! Claude Code 2.1.310 takes a user message written to stdin while a turn runs into the node's
//! own conversation right after the tool call in progress, whatever its size and also while a
//! subagent works, and reports it on stdout when it does. A message still queued when a turn
//! ends starts a further turn with a result of its own.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::mpsc;

use crate::active_memory::hooks::ActiveMemoryHooks;
use crate::native_v2_capsule::provider_process::LiveInput;

/// Maximum wait for a sent steer's replay acknowledgement after a turn result. Closing stdin
/// then leaves delivery unconfirmed: the steer has no receipt and remains due.
const UNREPORTED_AFTER_RESULT: Duration = if cfg!(test) {
    Duration::from_millis(500)
} else {
    Duration::from_secs(60)
};

/// What the stdin side of a turn learns from stdout.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum TurnSignal {
    /// Claude Code took this user message from stdin into the conversation.
    Replayed(String),
    /// A main-loop turn ended with a result.
    Result,
}

pub(super) fn user_message(text: &str) -> Vec<u8> {
    let mut line = serde_json::json!({
        "type":"user", "message":{"role":"user", "content":[{"type":"text", "text":text}]},
    })
    .to_string()
    .into_bytes();
    line.push(b'\n');
    line
}

pub(super) struct LiveTurn {
    /// Steers selected at this execution's tool calls.
    pub(super) steers: mpsc::UnboundedReceiver<String>,
    pub(super) signals: mpsc::UnboundedReceiver<TurnSignal>,
    pub(super) hooks: Option<Arc<ActiveMemoryHooks>>,
    pub(super) execution: u64,
}

/// Stdin after the prompt. It closes after a result with every sent steer reported, when stdout
/// ends, or when the post-result acknowledgement deadline expires. The deadline bounds a wait
/// for missing replay acknowledgements without receipting the pending steers.
pub(super) fn live(turn: LiveTurn) -> LiveInput<'static> {
    let state = State {
        turn,
        unreported: Vec::new(),
        steers_open: true,
        ended: None,
    };
    futures_util::stream::unfold(state, |mut state| async move {
        let message = state.next().await?;
        Some((message, state))
    })
    .boxed()
}

struct State {
    turn: LiveTurn,
    /// Steers written to stdin that Claude Code has not reported yet.
    unreported: Vec<String>,
    steers_open: bool,
    /// When a turn ended while steers were unreported.
    ended: Option<tokio::time::Instant>,
}

impl State {
    async fn next(&mut self) -> Option<Vec<u8>> {
        loop {
            let unreported = async {
                match self.ended {
                    Some(ended) => tokio::time::sleep_until(ended + UNREPORTED_AFTER_RESULT).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                // A steer that is waiting when a result arrives is written first. In any other
                // order the result would close stdin over a steer its tool call handed over.
                biased;
                steer = self.turn.steers.recv(), if self.steers_open => match steer {
                    Some(text) => {
                        let line = user_message(&text);
                        self.unreported.push(text);
                        return Some(line);
                    }
                    None => self.steers_open = false,
                },
                signal = self.turn.signals.recv() => match signal? {
                    TurnSignal::Replayed(text) => self.reported(&text).await,
                    TurnSignal::Result if self.unreported.is_empty() => return None,
                    TurnSignal::Result => self.ended = Some(tokio::time::Instant::now()),
                },
                () = unreported => return None,
            }
        }
    }

    async fn reported(&mut self, text: &str) {
        let Some(at) = self.unreported.iter().position(|sent| sent == text) else {
            // The prompt, which is no steer.
            return;
        };
        self.unreported.remove(at);
        // The queued message starts or joins a turn whose result is still to come.
        self.ended = None;
        if let Some(hooks) = &self.turn.hooks {
            hooks.steer_shown(self.turn.execution, text).await;
        }
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod tests;
