use std::fmt;

const MAX_GITHUB_API_DIAGNOSTIC_BYTES: usize = 32 * 1024;

/// Bounded provider-owned GitHub API failure detail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitHubApiFailure {
    status: Option<u16>,
    diagnostic: Box<str>,
    transient: bool,
}

impl GitHubApiFailure {
    fn new(status: Option<u16>, diagnostic: impl Into<String>) -> Self {
        let mut diagnostic = diagnostic.into();
        if diagnostic.contains('\0') {
            diagnostic = "GitHub API diagnostic redacted".to_owned();
        }
        if diagnostic.len() > MAX_GITHUB_API_DIAGNOSTIC_BYTES {
            let mut boundary = MAX_GITHUB_API_DIAGNOSTIC_BYTES - "\n[diagnostic truncated]".len();
            while !diagnostic.is_char_boundary(boundary) {
                boundary = boundary.saturating_sub(1);
            }
            diagnostic.truncate(boundary);
            diagnostic.push_str("\n[diagnostic truncated]");
        }
        Self {
            status,
            diagnostic: diagnostic.into_boxed_str(),
            transient: false,
        }
    }

    fn retryable_review_sync(&self) -> bool {
        self.transient
            || self
                .status
                .is_some_and(|status| matches!(status, 404 | 409 | 422 | 429 | 500..=599))
    }

    fn authentication_failed(&self) -> bool {
        matches!(self.status, Some(401))
    }
}

impl fmt::Display for GitHubApiFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.diagnostic)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum GitHubAuthorityError {
    #[error("GitHub delivery authority is unavailable")]
    Unavailable,
    #[error("GitHub rejected delivery")]
    Rejected,
    #[error("GitHub delivery identity mismatch: {0}")]
    Identity(Box<str>),
    #[error("GitHub API request failed: {0}")]
    Api(GitHubApiFailure),
    #[error("GitHub delivery left repository work requiring repair: {0}")]
    Repairable(Box<str>),
    #[error("{0}")]
    Command(Box<super::GitCommandFailure>),
}

impl GitHubAuthorityError {
    pub(super) fn api_status(&self) -> Option<u16> {
        match self {
            Self::Api(failure) => failure.status,
            _ => None,
        }
    }

    pub(super) fn api(status: Option<u16>, diagnostic: impl Into<String>) -> Self {
        Self::Api(GitHubApiFailure::new(status, diagnostic))
    }

    pub(super) fn retryable_review_sync(&self) -> bool {
        match self {
            Self::Unavailable => true,
            Self::Rejected | Self::Identity(_) | Self::Repairable(_) | Self::Command(_) => false,
            Self::Api(failure) => failure.retryable_review_sync(),
        }
    }

    pub(super) fn authentication_failed(&self) -> bool {
        match self {
            Self::Api(failure) => failure.authentication_failed(),
            Self::Command(failure) => failure.authentication_failed(),
            Self::Unavailable | Self::Rejected | Self::Identity(_) | Self::Repairable(_) => false,
        }
    }

    pub(super) fn retryable_operation(&self) -> bool {
        match self {
            Self::Unavailable => true,
            Self::Api(failure) => {
                failure.transient
                    || failure
                        .status
                        .is_some_and(|status| matches!(status, 429 | 500..=599))
            }
            Self::Command(failure) => failure.retryable_transport(),
            Self::Rejected | Self::Identity(_) | Self::Repairable(_) => false,
        }
    }

    pub(super) fn temporary(mut self) -> Self {
        if let Self::Api(failure) = &mut self {
            failure.transient = true;
        }
        self
    }

    pub(super) fn identity(diagnostic: impl Into<String>) -> Self {
        Self::Identity(GitHubApiFailure::new(None, diagnostic).diagnostic)
    }

    pub(super) fn with_context(self, context: impl fmt::Display) -> Self {
        match self {
            Self::Api(failure) => {
                let mut wrapped =
                    GitHubApiFailure::new(failure.status, format!("{context}\n{failure}"));
                wrapped.transient = failure.transient;
                Self::Api(wrapped)
            }
            Self::Identity(detail) => Self::identity(format!("{context}\n{detail}")),
            Self::Repairable(detail) => Self::repairable(format!("{context}\n{detail}")),
            Self::Unavailable => Self::api(
                None,
                format!("{context}\nGitHub delivery authority is unavailable"),
            )
            .temporary(),
            Self::Rejected => Self::api(None, format!("{context}\nGitHub rejected delivery")),
            Self::Command(failure) => Self::Command(Box::new(failure.with_context(context))),
        }
    }

    pub(super) fn review_head_not_visible() -> Self {
        Self::api(None, "GitHub review head revision is not visible").temporary()
    }

    pub(super) fn repairable(diagnostic: impl Into<String>) -> Self {
        Self::Repairable(GitHubApiFailure::new(None, diagnostic).diagnostic)
    }
}

impl From<super::GitCommandFailure> for GitHubAuthorityError {
    fn from(failure: super::GitCommandFailure) -> Self {
        Self::Command(Box::new(failure))
    }
}
