use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitCommandFailure {
    pub(crate) command: Box<str>,
    pub(crate) working_directory: Box<str>,
    pub(crate) exit_status: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
    pub(crate) context: Box<str>,
    pub(crate) timed_out: bool,
    pub(crate) cleanup_unconfirmed: bool,
    pub(crate) classification: Option<GitCommandClassification>,
}

/// Secret-free control evidence captured before diagnostic redaction changes Git text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GitCommandClassification {
    http_status: Option<u16>,
    authentication_rejection: bool,
    transient_transport_failure: bool,
}

impl fmt::Display for GitCommandFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "command: {}\nworkingDirectory: {}\nexitStatus: {:?}\n{}\nstdout{}:\n{}\nstderr{}:\n{}",
            self.command,
            self.working_directory,
            self.exit_status,
            self.context,
            if self.stdout_truncated {
                " (truncated)"
            } else {
                ""
            },
            self.stdout,
            if self.stderr_truncated {
                " (truncated)"
            } else {
                ""
            },
            self.stderr
        )
    }
}

impl std::error::Error for GitCommandFailure {}

impl GitCommandFailure {
    pub(crate) fn with_context(mut self, context: impl fmt::Display) -> Self {
        self.context = format!("{}\n{context}", self.context).into_boxed_str();
        self
    }

    pub(crate) fn classify_stderr(&mut self, stderr: &str) {
        self.classification = Some(GitCommandClassification::from_stderr(stderr));
    }

    fn stderr_classification(&self) -> GitCommandClassification {
        self.classification
            .unwrap_or_else(|| GitCommandClassification::from_stderr(&self.stderr))
    }

    pub(crate) fn http_status(&self) -> Option<u16> {
        self.stderr_classification().http_status
    }

    pub(crate) fn authentication_failed(&self) -> bool {
        self.http_status() == Some(401) || self.stderr_classification().authentication_rejection
    }

    pub(crate) fn retryable_transport(&self) -> bool {
        !self.cleanup_unconfirmed
            && (self.timed_out
                || self
                    .http_status()
                    .is_some_and(|status| matches!(status, 429 | 500..=599))
                || self.stderr_classification().transient_transport_failure)
    }

    pub(crate) fn operator_stderr(&self) -> String {
        format!(
            "stderr{}:\n{}\ncommand: {}\nworkingDirectory: {}\nexitStatus: {:?}\n{}",
            if self.stderr_truncated {
                " (truncated)"
            } else {
                ""
            },
            self.stderr,
            self.command,
            self.working_directory,
            self.exit_status,
            self.context,
        )
    }

    pub(crate) fn require_success(self) -> Result<Self, Self> {
        if self.exit_status == Some(0) && !self.cleanup_unconfirmed {
            Ok(self)
        } else {
            Err(self)
        }
    }
}

impl GitCommandClassification {
    fn from_stderr(stderr: &str) -> Self {
        let http_status = stderr.lines().find_map(|line| {
            line.split_once("The requested URL returned error: ")
                .and_then(|(_, status)| status.split_whitespace().next())
                .and_then(|status| status.parse().ok())
        });
        let authentication_rejection = stderr.lines().any(|line| {
            line.starts_with("fatal: Authentication failed for ")
                || terminal_http_auth_prompt(line)
                || line.contains("Permission denied (publickey")
        });
        let transient_transport_failure = stderr.lines().any(transient_transport_line);
        Self {
            http_status,
            authentication_rejection,
            transient_transport_failure,
        }
    }
}

fn transient_transport_line(line: &str) -> bool {
    line.starts_with("fatal: unable to access ")
        && [
            "Could not resolve host:",
            "Failed to connect to ",
            "Connection timed out",
            "Recv failure: Connection reset by peer",
            "Empty reply from server",
        ]
        .iter()
        .any(|message| line.contains(message))
}

// With terminal prompts disabled, Git reports an HTTP authentication challenge this way
// when it has no credential-helper username, even if the rejected token used extraHeader.
fn terminal_http_auth_prompt(line: &str) -> bool {
    let Some(prompt) = line.strip_prefix("fatal: could not read ") else {
        return false;
    };
    let remote = prompt
        .strip_prefix("Username for '")
        .or_else(|| prompt.strip_prefix("Password for '"));
    remote.is_some_and(|remote| {
        (remote.starts_with("http://") || remote.starts_with("https://"))
            && remote.ends_with("': terminal prompts disabled")
    })
}
