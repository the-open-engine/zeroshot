use crate::commands::types::{CommandError, ParsedCommand};

pub fn parse(raw: &str) -> Result<ParsedCommand, CommandError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CommandError::new("Enter a command."));
    }
    if !trimmed.starts_with('/') {
        return Err(CommandError::new("Commands must start with '/'."));
    }

    let body = trimmed.trim_start_matches('/').trim();
    if body.is_empty() {
        return Err(CommandError::new("Enter a command after '/'."));
    }

    let mut parts = body.split_whitespace();
    let Some(name) = parts.next() else {
        return Err(CommandError::new("Enter a command after '/'."));
    };

    let args = parts.map(|part| part.to_string()).collect::<Vec<_>>();
    Ok(ParsedCommand {
        raw: trimmed.to_string(),
        name: name.to_lowercase(),
        args,
    })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parse_empty() {
        let err = parse("").expect_err("expected error");
        assert_eq!(err.to_string(), "Enter a command.");
    }

    #[test]
    fn parse_slash_only() {
        let err = parse("/").expect_err("expected error");
        assert_eq!(err.to_string(), "Enter a command after '/'.");
    }

    #[test]
    fn parse_whitespace_tolerant() {
        let parsed = parse("  /provider   codex  ").expect("expected command");
        assert_eq!(parsed.name, "provider");
        assert_eq!(parsed.args, vec!["codex".to_string()]);
    }

    #[test]
    fn parse_provider_command() {
        let parsed = parse("/provider codex").expect("expected command");
        assert_eq!(parsed.name, "provider");
        assert_eq!(parsed.args, vec!["codex".to_string()]);
    }

    #[test]
    fn parse_issue_command() {
        let parsed = parse("/issue org/repo#123").expect("expected command");
        assert_eq!(parsed.name, "issue");
        assert_eq!(parsed.args, vec!["org/repo#123".to_string()]);
    }
}
