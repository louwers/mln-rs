use mln_util::constants::ASSET_PROTOCOL;
use mln_util::pool::Pool;

use crate::storage::file_source::FileSource;
use crate::storage::local_file_request::local_file_response;
use crate::storage::request::Responder;
use crate::storage::resource::Resource;
use crate::storage::response::{Reason, ResourceError, Response};

fn accepts_url(url: &str) -> bool {
    url.starts_with(ASSET_PROTOCOL)
}

/// The asset at an `asset://` URL under `root`, or its error.
fn asset_response(root: &str, resource: &Resource) -> Response {
    if !accepts_url(&resource.url) {
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Other, "Invalid asset URL")));
        return response;
    }

    // Cut off the protocol and prefix with path.
    let url = &resource.url[ASSET_PROTOCOL.len()..];
    #[cfg(target_os = "android")]
    let url = crate::storage::android::url_path(url);
    let mut path = root.as_bytes().to_vec();
    if !path.ends_with(b"/") {
        path.push(b'/');
    }
    path.extend(percent_encoding::percent_decode_str(url));
    #[cfg(target_os = "android")]
    if let Some(name) = crate::storage::android::app_asset_name(&path) {
        return crate::storage::android::asset_response(name, resource.data_range);
    }
    local_file_response(&path, resource.data_range)
}

/// The `Asset` file source: `asset://` URLs, relative to an asset directory,
/// read on a thread pool. On Android, the files under
/// `crate::storage::android::APP_ASSETS` are the app's assets, and a query or
/// fragment ends the URL's path.
pub(crate) struct AssetFileSource {
    root: std::sync::Arc<str>,
    pool: Pool,
}

impl AssetFileSource {
    /// Loads `asset://` URLs from the directory `asset_path`, on `pool`.
    #[must_use]
    pub(crate) fn new(asset_path: &str, pool: Pool) -> Self {
        AssetFileSource { root: asset_path.into(), pool }
    }
}

impl FileSource for AssetFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        let (root, resource) = (std::sync::Arc::clone(&self.root), resource.clone());
        self.pool.spawn(move || {
            if !responder.is_cancelled() {
                responder.send(asset_response(&root, &resource));
            }
        });
    }

    fn can_request(&self, resource: &Resource) -> bool {
        accepts_url(&resource.url)
    }
}
