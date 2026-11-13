/// A pending request. Dropping it cancels the request; a cancelled request's
/// callback never runs.
pub trait AsyncRequest {}
