//! The error type of the library's fallible calls.
//!
//! An [`Error`] pairs an [`ErrorKind`], which callers branch on, with a
//! message for people. The kinds are maplibre-native-ffi's `mln_status`
//! values, so a C binding maps one to the other. Failures that callers handle
//! case by case have their own types instead, such as a style's load error or
//! a resource request's error.

use serde::{Deserialize, Serialize};

/// What went wrong, as a caller would react to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorKind {
    /// An argument is invalid, such as malformed JSON or a value out of range.
    InvalidArgument,
    /// The call doesn't fit the current state, such as adding a layer to a
    /// style that hasn't loaded.
    InvalidState,
    /// The platform, the build or the render target lacks the feature.
    Unsupported,
    /// The library or the platform hit an error, such as a malformed tile or
    /// a failed graphics driver call.
    Native,
    /// The work stopped before it finished, such as when its owner went away.
    Cancelled,
    /// Another call is still using the same object.
    Busy,
    /// The render target can't be drawn to anymore.
    TargetLost,
    /// Nothing has the given ID.
    NotFound,
    /// Something already has the given ID.
    AlreadyExists,
}

/// A failed call: its [`ErrorKind`] and a message that says what failed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Error { kind, message: message.into() }
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::InvalidArgument, message)
    }

    pub fn invalid_state(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::InvalidState, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Unsupported, message)
    }

    pub fn native(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Native, message)
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Cancelled, message)
    }

    pub fn busy(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::Busy, message)
    }

    pub fn target_lost(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::TargetLost, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::NotFound, message)
    }

    pub fn already_exists(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::AlreadyExists, message)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_the_message() {
        let error = Error::invalid_state("Style is not done loading.");
        assert_eq!(error.kind, ErrorKind::InvalidState);
        assert_eq!(error.to_string(), "Style is not done loading.");
    }
}
