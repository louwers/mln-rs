use std::sync::Arc;
use std::time::Duration;

use mln_util::chrono::Timestamp;

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

/// A response to a resource request. `Clone` shares the data.
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

/// What caching rules allow for a copy of a resource (RFC 9111, RFC 5861):
/// when it stops being fresh, and how it may be used once it is stale.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caching {
    /// When it stops being fresh; `None` if it never does.
    pub expires: Option<Timestamp>,
    /// Once stale, it is never used without revalidating it, not even in
    /// place of an error.
    pub must_revalidate: bool,
    /// Until when it may be used stale while it revalidates.
    pub stale_while_revalidate: Option<Timestamp>,
    /// Until when it may be used stale when revalidating it fails, and so
    /// while it revalidates. Without it, a stale copy may be used for as
    /// long as it needn't revalidate first.
    pub stale_if_error: Option<Timestamp>,
}

/// How a cached copy may be used now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reuse {
    /// It is fresh: use it.
    Fresh,
    /// It is stale, within its `stale-while-revalidate`: the server lets it
    /// be used while it revalidates.
    WhileRevalidating,
    /// It is stale, within its `stale-if-error` or without one: the server
    /// lets it stand in when revalidating it fails. The map shows it while
    /// it revalidates, too ([`Caching::shows_while_revalidating`]).
    OnError,
    /// It is stale and may not be used at all until it revalidates.
    Never,
}

impl Caching {
    /// How the copy may be used at `now`.
    #[must_use]
    pub fn reuse(&self, now: Timestamp) -> Reuse {
        if self.expires.is_none_or(|expires| expires > now) {
            Reuse::Fresh
        } else if self.must_revalidate {
            Reuse::Never
        } else if self.stale_while_revalidate.is_some_and(|until| until > now) {
            Reuse::WhileRevalidating
        } else if self.stale_if_error.is_none_or(|until| until > now) {
            Reuse::OnError
        } else {
            Reuse::Never
        }
    }

    /// Whether the copy may show at `now` while it revalidates: unless it
    /// may not be used at all. A stale copy that the server lets stand in for
    /// a failure shows at once, as RFC 9111 §5.2.1.2 lets a client accept
    /// (`max-stale`): an old map that updates beats a blank one while the
    /// network answers, and it would show if the network failed anyway.
    #[must_use]
    pub fn shows_while_revalidating(&self, now: Timestamp) -> bool {
        self.reuse(now) != Reuse::Never
    }

    /// Until when the copy may stand in when the server can't be reached;
    /// `None` if it always may.
    #[must_use]
    pub fn usable_offline_until(&self) -> Option<Timestamp> {
        let expires = self.expires?;
        if self.must_revalidate { Some(expires) } else { self.stale_if_error.map(|until| until.max(expires)) }
    }

    /// The rules once the client accepts the copy for up to `max_stale` past
    /// its expiry in place of an error (RFC 9111 §5.2.1.2): that widens a
    /// `stale-if-error` window, which otherwise limits how long it stands in.
    /// `must-revalidate` still forbids using it stale.
    #[must_use]
    pub fn with_max_stale(mut self, max_stale: Duration) -> Self {
        if let (Some(expires), Some(until), false) = (self.expires, self.stale_if_error, self.must_revalidate) {
            self.stale_if_error = Some(until.max(after(expires, max_stale)));
        }
        self
    }
}

/// `duration` after `time`, at most the last timestamp.
fn after(time: Timestamp, duration: Duration) -> Timestamp {
    let secs = i64::try_from(duration.as_secs()).unwrap_or(i64::MAX);
    Timestamp::from_secs(time.secs_since_epoch().saturating_add(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> Timestamp {
        Timestamp::from_secs(1000 + secs)
    }

    #[test]
    fn reuse() {
        let now = at(0);
        let stale = Caching { expires: Some(at(-10)), ..Caching::default() };
        assert_eq!(Caching::default().reuse(now), Reuse::Fresh);
        assert_eq!(Caching { expires: Some(at(10)), must_revalidate: true, ..stale }.reuse(now), Reuse::Fresh);
        assert_eq!(stale.reuse(now), Reuse::OnError);
        assert_eq!(Caching { must_revalidate: true, ..stale }.reuse(now), Reuse::Never);
        let revalidating = Caching { stale_while_revalidate: Some(at(5)), ..stale };
        assert_eq!(revalidating.reuse(now), Reuse::WhileRevalidating);
        assert_eq!(Caching { stale_while_revalidate: Some(at(0)), ..stale }.reuse(now), Reuse::OnError);
        assert_eq!(Caching { must_revalidate: true, ..revalidating }.reuse(now), Reuse::Never);
        assert_eq!(Caching { stale_if_error: Some(at(5)), ..stale }.reuse(now), Reuse::OnError);
        assert_eq!(Caching { stale_if_error: Some(at(0)), ..stale }.reuse(now), Reuse::Never);
    }

    #[test]
    fn shows_while_revalidating() {
        let now = at(0);
        let stale = Caching { expires: Some(at(-10)), ..Caching::default() };
        assert!(Caching::default().shows_while_revalidating(now));
        assert!(stale.shows_while_revalidating(now));
        assert!(Caching { stale_if_error: Some(at(5)), ..stale }.shows_while_revalidating(now));
        assert!(!Caching { stale_if_error: Some(at(-5)), ..stale }.shows_while_revalidating(now));
        assert!(!Caching { must_revalidate: true, ..stale }.shows_while_revalidating(now));
        assert!(Caching { expires: Some(at(10)), must_revalidate: true, ..stale }.shows_while_revalidating(now));
    }

    #[test]
    fn usable_offline_until() {
        assert_eq!(Caching::default().usable_offline_until(), None);
        let expired = Caching { expires: Some(at(-10)), ..Caching::default() };
        assert_eq!(expired.usable_offline_until(), None);
        assert_eq!(Caching { must_revalidate: true, ..expired }.usable_offline_until(), Some(at(-10)));
        assert_eq!(Caching { stale_if_error: Some(at(5)), ..expired }.usable_offline_until(), Some(at(5)));
    }

    #[test]
    fn max_stale_widens_stale_if_error() {
        let day = Duration::from_hours(24);
        let if_error = Caching { expires: Some(at(-10)), stale_if_error: Some(at(-5)), ..Caching::default() };
        let widened = if_error.with_max_stale(day);
        assert_eq!(widened.stale_if_error, Some(at(86_390)));
        assert_eq!(widened.reuse(at(0)), Reuse::OnError);
        assert_eq!(if_error.with_max_stale(Duration::ZERO), if_error);
        assert_eq!(if_error.with_max_stale(Duration::MAX).stale_if_error, Some(Timestamp::from_secs(i64::MAX)));
        let unlimited = Caching { stale_if_error: None, ..if_error };
        assert_eq!(unlimited.with_max_stale(day), unlimited);
        let must_revalidate = Caching { must_revalidate: true, ..if_error };
        assert_eq!(must_revalidate.with_max_stale(day), must_revalidate);
    }
}
