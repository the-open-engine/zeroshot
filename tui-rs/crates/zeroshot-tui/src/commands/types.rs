use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCommand {
    pub raw: String,
    pub name: String,
    pub args: Vec<String>,
}

impl ParsedCommand {
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn args(&self) -> &[String] {
        self.args.as_slice()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

pub const VALID_PROVIDERS: [&str; 4] = ["claude", "codex", "gemini", "opencode"];
