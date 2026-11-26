use std::sync::Arc;

use crate::storage::chrono::Timestamp;

/// A shared response body, as bytes.
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
    pub(crate) retry_after: Option<Timestamp>,
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
    /// Why the request failed; `None` when it succeeded.
    pub error: Option<Box<ResourceError>>,

    /// The response has no body: a 204 No Content, or a 404 Not Found for a
    /// tile.
    pub no_content: bool,

    /// A 304 Not Modified: the requester's copy is still current.
    pub not_modified: bool,

    /// The data is the copy the request carried (`Resource::prior_data`),
    /// which a 304 delivered: a cache that stored that copy has its data
    /// already.
    pub prior: bool,

    /// Set, with `no_content`, for a 404 Not Found for a tile, which the
    /// tileset doesn't have, so a tile loader can tell it apart from a tile
    /// with nothing in it.
    pub tile_not_found: bool,

    /// The body, except for an error, a 304 or no content.
    pub data: Option<Data>,

    pub modified: Option<Timestamp>,
    pub etag: Option<String>,

    /// How the data may be cached and reused. A response that never expires,
    /// as a local file's, has the default rules.
    pub caching: Caching,
    /// The response must not be stored.
    pub no_store: bool,
}

impl Response {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// How a copy of a resource may be reused (RFC 9111): when it stops being
/// fresh, and whether it may be used once it is stale.
///
/// The map accepts a stale copy of any age, as RFC 9111 §5.2.1.2 lets a
/// client (`max-stale` without a value): it shows the copy while it
/// revalidates, and while the server can't be reached. Only the server can
/// forbid that, with `must-revalidate` or `no-cache`. Its
/// `stale-while-revalidate` and `stale-if-error` (RFC 5861) permit what the
/// map accepts anyway, so they aren't read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caching {
    /// When it stops being fresh; `None` if it never does.
    pub expires: Option<Timestamp>,
    /// Once stale, it may not be used until it revalidates, not even while
    /// the server can't be reached.
    pub must_revalidate: bool,
}

impl Caching {
    /// Whether the copy is fresh at `now`: it may be used without asking the
    /// server.
    #[must_use]
    pub fn is_fresh(&self, now: Timestamp) -> bool {
        self.expires.is_none_or(|expires| expires > now)
    }

    /// Whether the copy may be used at `now`: it is fresh, or it may be used
    /// stale.
    #[must_use]
    pub fn is_usable(&self, now: Timestamp) -> bool {
        !self.must_revalidate || self.is_fresh(now)
    }

    /// Until when the copy may be used; `None` if it always may.
    #[must_use]
    pub fn usable_until(&self) -> Option<Timestamp> {
        if self.must_revalidate { self.expires } else { None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(1000 + secs)
    }

    #[test]
    fn a_stale_copy_is_usable_unless_it_must_revalidate() {
        let now = at(0);
        let stale = Caching { expires: Some(at(-10)), must_revalidate: false };
        let must_revalidate = Caching { must_revalidate: true, ..stale };
        assert!(Caching::default().is_fresh(now) && Caching::default().is_usable(now));
        assert!(!stale.is_fresh(now) && stale.is_usable(now));
        assert!(!must_revalidate.is_usable(now));
        assert!(Caching { expires: Some(at(10)), ..must_revalidate }.is_usable(now));
        assert_eq!(stale.usable_until(), None);
        assert_eq!(must_revalidate.usable_until(), Some(at(-10)));
    }
}
