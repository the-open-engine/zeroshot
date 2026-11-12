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
    policy_rejection: bool,
    non_fast_forward: bool,
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

    pub(crate) fn classify_stdout(&mut self, stdout: &str) {
        let mut classification = self.stderr_classification();
        classification.non_fast_forward = stdout.lines().any(|line| {
            line.starts_with("!\t")
                && line.contains("[rejected]")
                && (line.ends_with("(non-fast-forward)") || line.ends_with("(fetch first)"))
        });
        self.classification = Some(classification);
    }

    pub(crate) fn non_fast_forward(&self) -> bool {
        self.stderr_classification().non_fast_forward
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

    /// The forge definitively refused the push by branch protection or push permission, which no
    /// candidate change can satisfy.
    pub(crate) fn policy_rejected(&self) -> bool {
        self.stderr_classification().policy_rejection
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
        let policy_rejection = stderr.lines().any(policy_rejection_line);
        Self {
            http_status,
            authentication_rejection,
            transient_transport_failure,
            policy_rejection,
            non_fast_forward: false,
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

// Git relays server text as `remote: ` lines, padded with trailing spaces; GitLab's SSH shell
// writes that prefix itself, and its pre-receive check adds `GitLab: `. GitHub's SSH frontend
// refuses before Git starts, so its own `ERROR: ` lines arrive unprefixed. Only these
// forge-authored refusals are definitive; a generic hook decline or `[remote rejected]` status
// alone may describe repairable content.
fn policy_rejection_line(line: &str) -> bool {
    if let Some(message) = line.strip_prefix("ERROR: ") {
        return message
            .starts_with("The key you are authenticating with has been marked as read only.")
            || github_permission_denial(message);
    }
    let Some(message) = line.strip_prefix("remote: ") else {
        return false;
    };
    let gitlab = message.strip_prefix("GitLab: ").unwrap_or(message);
    [
        // GitHub branch protection, ruleset ref restrictions and push permission.
        "error: GH006: Protected branch update failed for ",
        "- Cannot create ref due to creations being restricted.",
        "- Cannot update this protected ref.",
        "Write access to repository not granted.",
    ]
    .iter()
    .any(|refusal| message.starts_with(refusal))
        || github_permission_denial(message)
        || [
            // GitLab protected branches (`Gitlab::Checks::BranchCheck`).
            "You are not allowed to push code to protected branches on this project.",
            "You are not allowed to create protected branches on this project.",
            "You can only use an existing protected branch ref as the basis of a new protected \
             branch.",
            "You can only create protected branches using the web interface and API.",
            // GitLab push permission (`Gitlab::GitAccess`).
            "You are not allowed to push code to this project.",
            "You are not allowed to upload code for this project.",
            "You are not allowed to upload code.",
            "This deploy key does not have write access to this project.",
        ]
        .iter()
        .any(|refusal| gitlab.starts_with(refusal))
}

// `Permission to OWNER/REPO.git denied to ACTOR.`, where a deploy key's ACTOR is `OWNER/REPO`.
fn github_permission_denial(message: &str) -> bool {
    message
        .strip_prefix("Permission to ")
        .and_then(|denial| denial.split_once(" denied to "))
        .is_some_and(|(repository, actor)| {
            repository.contains('/')
                && !repository.contains(char::is_whitespace)
                && !actor.trim().is_empty()
        })
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
