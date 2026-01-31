use std::fmt;

use crate::app::{Action, CommandRequest};

#[derive(Debug, Clone)]
pub struct CommandError {
    message: String,
}

impl CommandError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for CommandError {}

pub fn dispatch(request: CommandRequest) -> Result<Vec<Action>, CommandError> {
    match request {
        CommandRequest::SubmitRaw { raw } => Err(CommandError::new(format!(
            "Command handling not implemented yet: {raw}"
        ))),
    }
}
