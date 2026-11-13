use std::sync::Arc;

use crate::resource::Kind;
use crate::response::ResourceError;

/// What a resource transform answers: the URL to load instead, or the error
/// to fail the request with. A failed request is never sent. When the
/// error's reason allows a retry, the retry calls the transform again.
pub type TransformResult = Result<String, ResourceError>;

/// The callback a resource transform answers through, once.
pub type FinishedCallback = Box<dyn FnOnce(TransformResult) + Send>;

/// Rewrites request URLs (asynchronously) before the online file source
/// requests them, or fails the requests. It gets the resource's kind and
/// URL, and runs on the online file source's thread, hence `Send + Sync`.
pub type ResourceTransform = Arc<dyn Fn(Kind, &str, FinishedCallback) + Send + Sync>;
