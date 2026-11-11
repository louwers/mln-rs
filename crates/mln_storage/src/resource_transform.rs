// Ports include/mln/storage/resource_transform.hpp and
// src/mln/storage/resource_transform.cpp, with maplibre-native-ffi's patch 0068
// (a transform can fail a request).

use std::sync::Arc;

use crate::resource::Kind;
use crate::response::Error;

/// `ResourceTransform::Result`: the URL to load instead, or the error to fail
/// the request with. A failed request is never sent. When the error's reason
/// allows a retry, the retry calls the transform again.
pub type TransformResult = Result<String, Error>;

/// `ResourceTransform::FinishedCallback` (`std::function<void(Result)>`).
pub type FinishedCallback = Box<dyn FnOnce(TransformResult) + Send>;

/// `ResourceTransform::TransformCallback`. It runs on the online file
/// source's thread, hence `Send + Sync`.
pub type TransformCallback = Arc<dyn Fn(Kind, &str, FinishedCallback) + Send + Sync>;

/// `ResourceTransform`: rewrites request URLs (asynchronously) before the
/// online file source requests them, or fails the requests.
#[derive(Clone, Default)]
pub struct ResourceTransform {
    transform_callback: Option<TransformCallback>,
}

impl ResourceTransform {
    /// `ResourceTransform(TransformCallback = {})`.
    pub fn new(callback: Option<TransformCallback>) -> Self {
        ResourceTransform { transform_callback: callback }
    }

    /// `transform(Resource::Kind, const std::string& url, FinishedCallback)`.
    pub fn transform(&self, kind: Kind, url: &str, finished: FinishedCallback) {
        let callback = self.transform_callback.as_ref().expect("transformCallback");
        callback(kind, url, finished);
    }

    /// `explicit operator bool()`.
    pub fn is_set(&self) -> bool {
        self.transform_callback.is_some()
    }
}
