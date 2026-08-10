use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("keychain error: {0}")]
    Keyring(#[from] keyring::Error),
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    /// `status` is the HTTP code when the failure came off the wire. Callers
    /// that recover from a specific one (406 → paginate, 422 → re-anchor)
    /// match on it rather than grepping the formatted message for a number,
    /// which matched PR #422 and any body containing the word.
    #[error("GitHub API error: {message}")]
    GitHub { status: Option<u16>, message: String },
    #[error("{0}")]
    Other(String),
}

impl AppError {
    pub fn github(message: impl Into<String>) -> Self {
        AppError::GitHub { status: None, message: message.into() }
    }

    pub fn github_status(status: u16, message: impl Into<String>) -> Self {
        AppError::GitHub { status: Some(status), message: message.into() }
    }

    /// The HTTP status behind a GitHub failure, when one was reported.
    pub fn status(&self) -> Option<u16> {
        match self {
            AppError::GitHub { status, .. } => *status,
            _ => None,
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
