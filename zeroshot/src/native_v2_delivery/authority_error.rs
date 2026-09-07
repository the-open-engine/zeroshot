use std::fmt;

const MAX_GITHUB_API_DIAGNOSTIC_BYTES: usize = 1024;

/// Bounded provider-owned GitHub API failure detail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitHubApiFailure {
    status: Option<u16>,
    diagnostic: Box<str>,
}

impl GitHubApiFailure {
    fn new(status: Option<u16>, diagnostic: impl Into<String>) -> Self {
        let mut diagnostic = diagnostic.into();
        if diagnostic.contains('\0') {
            diagnostic = "GitHub API diagnostic redacted".to_owned();
        }
        if diagnostic.len() > MAX_GITHUB_API_DIAGNOSTIC_BYTES {
            let mut boundary = MAX_GITHUB_API_DIAGNOSTIC_BYTES;
            while !diagnostic.is_char_boundary(boundary) {
                boundary = boundary.saturating_sub(1);
            }
            diagnostic.truncate(boundary);
        }
        Self {
            status,
            diagnostic: diagnostic.into_boxed_str(),
        }
    }

    fn retryable_review_sync(&self) -> bool {
        self.status
            .is_none_or(|status| matches!(status, 404 | 409 | 422 | 429 | 500..=599))
    }

    fn authentication_failed(&self) -> bool {
        matches!(self.status, Some(401 | 403))
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
    #[error("GitHub API request failed: {0}")]
    Api(GitHubApiFailure),
}

impl GitHubAuthorityError {
    pub(super) fn api(status: Option<u16>, diagnostic: impl Into<String>) -> Self {
        Self::Api(GitHubApiFailure::new(status, diagnostic))
    }

    pub(super) fn retryable_review_sync(&self) -> bool {
        match self {
            Self::Unavailable => true,
            Self::Rejected => false,
            Self::Api(failure) => failure.retryable_review_sync(),
        }
    }

    pub(super) fn authentication_failed(&self) -> bool {
        matches!(self, Self::Api(failure) if failure.authentication_failed())
    }

    pub(super) fn review_head_not_visible() -> Self {
        Self::api(None, "GitHub review head revision is not visible")
    }
}
