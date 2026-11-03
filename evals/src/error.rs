//! The error type shared by the eval modules.

use std::fmt;

/// Why a search reply could not be read as a ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeProblem {
    /// There was no structured content and the text block was not JSON.
    NotJson,
    /// The reply had no list under any of the recognised keys.
    NoResultList,
}

impl fmt::Display for ShapeProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ShapeProblem::NotJson => "the tool result is neither structured content nor JSON text",
            ShapeProblem::NoResultList => {
                "the tool result has no list under results, products, items or hits"
            }
        })
    }
}

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
    /// The server replied with a JSON-RPC error object.
    Rpc {
        /// The JSON-RPC method that was called.
        method: String,
        /// The JSON-RPC error code.
        code: i64,
        /// The server's error message.
        message: String,
    },
    /// A search reply could not be read as a ranking.
    Shape(ShapeProblem),
    /// The model API refused the request, after any retries.
    Api {
        /// HTTP status of the last attempt.
        status: u16,
        /// The API's error message, with the key redacted.
        message: String,
    },
}

/// Result alias for the eval modules.
pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Http(e) => write!(f, "HTTP error: {e}"),
            Error::Json(e) => write!(f, "invalid JSON: {e}"),
            Error::Input(m) | Error::Protocol(m) => f.write_str(m),
            Error::Rpc {
                method,
                code,
                message,
            } => write!(f, "{method}: JSON-RPC error {code}: {message}"),
            Error::Shape(problem) => problem.fmt(f),
            Error::Api { status, message } => {
                write!(f, "the API answered HTTP {status}: {message}")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Http(e) => Some(e),
            Error::Json(e) => Some(e),
            _ => None,
        }
    }
}

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
