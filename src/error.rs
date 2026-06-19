//! The crate's public error type, [`EvalError`].
//!
//! `eval-core` is built to lift into a standalone, publishable crate, so its PUBLIC fallible paths
//! ([`load_cases`](crate::load_cases), [`Agent::run`](crate::Agent::run), regex compilation in the
//! built-in scorer) surface a concrete, `std::error::Error`-implementing type rather than `anyhow`.
//! Internal helpers (e.g. the HTML report generator) may still use `anyhow` for convenience; only the
//! public signatures expose [`EvalError`].
//!
//! Every fallible third-party source has a `From` impl ([`std::io::Error`], [`ron::error::SpannedError`],
//! [`regex::Error`]) so the `?` operator threads cleanly, and the agent-side variant
//! ([`EvalError::Agent`]) lets a host wrap whatever its own run failure was as a string.

use std::path::PathBuf;

/// The error type for `eval-core`'s public API.
///
/// `#[non_exhaustive]` so new fallible paths can add variants without a breaking change. Construct the
/// host-facing variant via [`EvalError::agent`]; the I/O / parse / regex variants are produced by `?`
/// through the [`From`] impls below.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EvalError {
    /// An I/O error reading a case directory or file. Carries the offending path (when known) for a
    /// fail-loud message that names what couldn't be read.
    #[error("i/o error{}: {source}", .path.as_ref().map(|p| format!(" for {}", p.display())).unwrap_or_default())]
    Io {
        /// The path being read when the error occurred, if known.
        path: Option<PathBuf>,
        /// The underlying I/O error.
        source: std::io::Error,
    },

    /// A `.ron` case file failed to parse. Carries the offending file path so a typo fails loud.
    #[error("failed to parse RON case {}: {source}", .path.display())]
    RonParse {
        /// The `.ron` file that failed to deserialize.
        path: PathBuf,
        /// The underlying RON deserialization error (with span info).
        source: ron::error::SpannedError,
    },

    /// A regex in a [`FinalTextMatches`](crate::expect::Expectation::FinalTextMatches) expectation
    /// failed to compile.
    #[error("invalid regex `{pattern}`: {source}")]
    Regex {
        /// The pattern that failed to compile.
        pattern: String,
        /// The underlying regex compilation error.
        source: regex::Error,
    },

    /// The host's [`Agent::run`](crate::Agent::run) (or another host-supplied step) failed. Carries the
    /// host's own error rendered to a string — `eval-core` stays free of the host's error types.
    #[error("agent run failed: {0}")]
    Agent(String),
}

impl EvalError {
    /// Wrap a host-side run failure (anything `Display`) as an [`EvalError::Agent`]. The canonical way a
    /// host's [`Agent::run`](crate::Agent::run) reports a backend/loop failure to the generic runner.
    pub fn agent(error: impl std::fmt::Display) -> Self {
        EvalError::Agent(error.to_string())
    }
}

impl From<std::io::Error> for EvalError {
    fn from(source: std::io::Error) -> Self {
        EvalError::Io { path: None, source }
    }
}

impl From<ron::error::SpannedError> for EvalError {
    /// A bare `ron` error with no associated path. [`load_cases`](crate::load_cases) builds the
    /// path-carrying [`EvalError::RonParse`] directly, so this is the fallback for any other call site.
    fn from(source: ron::error::SpannedError) -> Self {
        EvalError::RonParse {
            path: PathBuf::new(),
            source,
        }
    }
}

impl From<regex::Error> for EvalError {
    fn from(source: regex::Error) -> Self {
        EvalError::Regex {
            pattern: String::new(),
            source,
        }
    }
}
