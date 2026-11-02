//! The error type shared by the eval modules.

use std::fmt;

/// Anything that can go wrong while running an eval.
#[derive(Debug)]
pub enum Error {
    /// The HTTP request failed before a usable response arrived.
    Http(reqwest::Error),
    /// A body was not the JSON that was expected.
    Json(serde_json::Error),
    /// A local file or option was unusable.
    Input(String),
    /// The server answered, but not the way MCP or the API says it should.
    Protocol(String),
}

/// Result alias for the eval modules.
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Http(e) => write!(f, "HTTP error: {e}"),
            Error::Json(e) => write!(f, "invalid JSON: {e}"),
            Error::Input(m) | Error::Protocol(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Http(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e)
    }
}
