use std::io;

use tokio::task::JoinError;

pub(crate) fn io_error_detail(operation: &'static str, error: &io::Error) -> String {
    let raw_os_error = error
        .raw_os_error()
        .map_or_else(|| "none".to_owned(), |code| code.to_string());
    let message = sanitize_message(&error.to_string());
    format!(
        "{operation}: kind={:?}, raw_os_error={raw_os_error}, message={message}",
        error.kind()
    )
}

pub(super) fn task_join_detail(operation: &'static str, error: &JoinError) -> String {
    let cause = if error.is_panic() {
        "task panicked"
    } else if error.is_cancelled() {
        "task was cancelled"
    } else {
        "task failed"
    };
    format!("{operation}: {cause}")
}

fn sanitize_message(message: &str) -> String {
    message
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "diagnostic/tests.rs"]
mod tests;
