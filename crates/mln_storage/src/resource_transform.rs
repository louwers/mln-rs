use std::sync::Arc;

use crate::resource::Kind;
use crate::response::ResourceError;

/// The URL to load instead, or the error to fail the request with. A failed
/// request is never sent. When the error's reason allows a retry, the retry
/// calls the transform again.
pub type TransformResult = Result<String, ResourceError>;

/// Receives a transform's result.
pub type FinishedCallback = Box<dyn FnOnce(TransformResult) + Send>;

/// Transforms a request of a kind and URL, passing the result to the
/// callback. It runs on the online file source's thread, hence
/// `Send + Sync`.
pub type TransformCallback = Arc<dyn Fn(Kind, &str, FinishedCallback) + Send + Sync>;

/// Rewrites request URLs (asynchronously) before the online file source
/// requests them, or fails the requests.
#[derive(Clone, Default)]
pub struct ResourceTransform {
    transform_callback: Option<TransformCallback>,
}

impl ResourceTransform {
    #[must_use]
    pub fn new(callback: Option<TransformCallback>) -> Self {
        ResourceTransform { transform_callback: callback }
    }

    pub fn transform(&self, kind: Kind, url: &str, finished: FinishedCallback) {
        let callback = self.transform_callback.as_ref().expect("transformCallback");
        callback(kind, url, finished);
    }

    #[must_use]
    pub fn is_set(&self) -> bool {
        self.transform_callback.is_some()
    }
}
