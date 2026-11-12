// Ports include/mln/storage/response.hpp and src/mln/storage/response.cpp.

use std::sync::Arc;

use mln_util::chrono::{Timestamp, now};

/// `std::shared_ptr<const std::string>`: response bodies are bytes.
pub type Data = Arc<Vec<u8>>;

/// What made a resource request fail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Reason {
    Success = 1,
    NotFound = 2,
    Server = 3,
    Connection = 4,
    RateLimit = 5,
    #[default]
    Other = 6,
}

/// Why a resource request failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ResourceError {
    pub reason: Reason,
    /// An error message from the request handler, e.g. a server message or a
    /// system message informing the user about the reason for the failure.
    pub message: String,
    pub retry_after: Option<Timestamp>,
}

impl ResourceError {
    pub fn new(reason: Reason, message: impl Into<String>) -> Self {
        ResourceError { reason, message: message.into(), retry_after: None }
    }

    pub fn with_retry_after(reason: Reason, message: impl Into<String>, retry_after: Option<Timestamp>) -> Self {
        ResourceError { reason, message: message.into(), retry_after }
    }
}

/// `Response`. `Clone` is the C++ copy constructor (the error is copied,
/// the data shared).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Response {
    /// When this object is empty, the response was successful.
    pub error: Option<Box<ResourceError>>,

    /// This is set to true for 204 Not Modified responses, and, for backward
    /// compatibility, for 404 Not Found responses for tiles.
    pub no_content: bool,

    /// This is set to true for 304 Not Modified responses.
    pub not_modified: bool,

    /// Port-only: set, with `no_content`, for a 404 Not Found response for a tile, which the
    /// tileset doesn't have. A tile loader can tell such a tile apart from one with nothing in it.
    pub tile_not_found: bool,

    /// This is set to true when the server requested that no expired
    /// resources be used by specifying "Cache-Control: must-revalidate".
    pub must_revalidate: bool,

    /// The actual data of the response. Present only for non-error,
    /// non-notModified responses.
    pub data: Option<Data>,

    pub modified: Option<Timestamp>,
    pub expires: Option<Timestamp>,
    pub etag: Option<String>,
}

impl Response {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `isFresh()`.
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        match self.expires {
            Some(expires) => expires > now(),
            None => self.error.is_none(),
        }
    }

    /// `isUsable()`: whether we are allowed to use this response according
    /// to HTTP caching rules. It may or may not be stale.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.must_revalidate || self.expires.is_some_and(|expires| expires > now())
    }
}
