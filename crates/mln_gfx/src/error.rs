//! The error of the calls that make devices and render targets.
//!
//! An [`Error`] pairs an [`ErrorKind`], which callers branch on, with a
//! message for people. The [`Gpu`](crate::gpu::Gpu) calls fail with
//! [`GpuError`](crate::gpu::GpuError) instead.

/// What went wrong, as a caller would react to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// An argument is invalid, such as a size the device can't make.
    InvalidArgument,
    /// The call doesn't fit the current state, such as a read before the
    /// first frame.
    InvalidState,
    /// The platform, the build or the render target lacks the feature.
    Unsupported,
    /// The platform or the graphics driver failed.
    Native,
    /// Nothing matches what the call names, such as a device on a GPU the
    /// host doesn't have.
    NotFound,
}

/// A failed call: its [`ErrorKind`] and a message that says what failed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{message}")]
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

    pub fn not_found(message: impl Into<String>) -> Self {
        Error::new(ErrorKind::NotFound, message)
    }
}
