//! The failure that a C entry point reports as an `mln_status` and a
//! diagnostic message.

use std::borrow::Cow;

use mln_util::ErrorKind;

use crate::generated::types::{
    MLN_STATUS_ALREADY_EXISTS, MLN_STATUS_BUSY, MLN_STATUS_CANCELLED, MLN_STATUS_INVALID_ARGUMENT,
    MLN_STATUS_INVALID_STATE, MLN_STATUS_NATIVE_ERROR, MLN_STATUS_NOT_FOUND, MLN_STATUS_TARGET_LOST,
    MLN_STATUS_UNSUPPORTED, MLN_STATUS_WRONG_THREAD, mln_status,
};

/// The `mln_status` that reports a failure of `kind`.
pub(crate) const fn status(kind: ErrorKind) -> mln_status {
    match kind {
        ErrorKind::InvalidArgument => MLN_STATUS_INVALID_ARGUMENT,
        ErrorKind::InvalidState => MLN_STATUS_INVALID_STATE,
        ErrorKind::Unsupported => MLN_STATUS_UNSUPPORTED,
        ErrorKind::Native => MLN_STATUS_NATIVE_ERROR,
        ErrorKind::Cancelled => MLN_STATUS_CANCELLED,
        ErrorKind::Busy => MLN_STATUS_BUSY,
        ErrorKind::TargetLost => MLN_STATUS_TARGET_LOST,
        ErrorKind::NotFound => MLN_STATUS_NOT_FOUND,
        ErrorKind::AlreadyExists => MLN_STATUS_ALREADY_EXISTS,
    }
}

/// A failure: the status a C function returns or a completion carries, and
/// the message its diagnostic shows.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub(crate) struct Error {
    status: mln_status,
    message: Cow<'static, str>,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<Cow<'static, str>>) -> Self {
        Self { status: status(kind), message: message.into() }
    }

    pub(crate) fn invalid_argument(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::InvalidArgument, message)
    }

    pub(crate) fn invalid_state(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::InvalidState, message)
    }

    pub(crate) fn native(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorKind::Native, message)
    }

    /// `MLN_STATUS_WRONG_THREAD`, which no core error reports: a call on a
    /// thread other than the one that an object belongs to.
    pub(crate) fn wrong_thread(message: impl Into<Cow<'static, str>>) -> Self {
        Self { status: MLN_STATUS_WRONG_THREAD, message: message.into() }
    }

    /// The status, never `MLN_STATUS_OK`.
    pub(crate) fn status(&self) -> mln_status {
        self.status
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl From<mln_util::Error> for Error {
    fn from(error: mln_util::Error) -> Self {
        Self::new(error.kind, error.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_maps_to_its_status() {
        let kinds = [
            (ErrorKind::InvalidArgument, MLN_STATUS_INVALID_ARGUMENT),
            (ErrorKind::InvalidState, MLN_STATUS_INVALID_STATE),
            (ErrorKind::Unsupported, MLN_STATUS_UNSUPPORTED),
            (ErrorKind::Native, MLN_STATUS_NATIVE_ERROR),
            (ErrorKind::Cancelled, MLN_STATUS_CANCELLED),
            (ErrorKind::Busy, MLN_STATUS_BUSY),
            (ErrorKind::TargetLost, MLN_STATUS_TARGET_LOST),
            (ErrorKind::NotFound, MLN_STATUS_NOT_FOUND),
            (ErrorKind::AlreadyExists, MLN_STATUS_ALREADY_EXISTS),
        ];
        for (kind, expected) in kinds {
            assert_eq!(status(kind), expected);
            assert_eq!(Error::new(kind, "message").status(), expected);
        }
    }

    #[test]
    fn a_core_error_keeps_its_kind_and_message() {
        let error = Error::from(mln_util::Error::not_found("no layer"));
        assert_eq!((error.status(), error.message()), (MLN_STATUS_NOT_FOUND, "no layer"));
    }
}
