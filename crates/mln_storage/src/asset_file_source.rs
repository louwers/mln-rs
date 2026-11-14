use mln_actor::{ActorRef, Thread};
use mln_util::constants::ASSET_PROTOCOL;

use crate::async_request::AsyncRequest;
use crate::file_source::{Callback, FileSource};
use crate::file_source_request::{FileSourceRequest, FileSourceRequestState};
use crate::local_file_request::request_local_file;
use crate::resource::Resource;
use crate::response::{Reason, ResourceError, Response};

fn accepts_url(url: &str) -> bool {
    url.starts_with(ASSET_PROTOCOL)
}

pub(crate) struct AssetFileSourceThread {
    root: String,
}

impl AssetFileSourceThread {
    pub(crate) fn request(&self, resource: &Resource, req: &ActorRef<FileSourceRequestState>) {
        if !accepts_url(&resource.url) {
            let mut response = Response::new();
            response.error = Some(Box::new(ResourceError::new(Reason::Other, "Invalid asset URL")));
            req.invoke(move |request: &FileSourceRequestState| request.set_response(response));
            return;
        }

        // Cut off the protocol and prefix with path.
        let url = &resource.url[ASSET_PROTOCOL.len()..];
        #[cfg(target_os = "android")]
        let url = crate::android::url_path(url);
        let mut path = self.root.clone().into_bytes();
        if !path.ends_with(b"/") {
            path.push(b'/');
        }
        path.extend(percent_encoding::percent_decode_str(url));
        #[cfg(target_os = "android")]
        if let Some(name) = crate::android::app_asset_name(&path) {
            crate::android::request_asset(name, req, resource.data_range);
            return;
        }
        request_local_file(&path, req, resource.data_range);
    }
}

/// The `Asset` file source: `asset://` URLs, relative to an asset directory. On Android, the files under
/// `mln_storage::android::APP_ASSETS` are the app's assets, and a query or
/// fragment ends the URL's path.
pub(crate) struct AssetFileSource {
    thread: Thread<AssetFileSourceThread>,
}

impl AssetFileSource {
    #[must_use]
    /// Loads `asset://` URLs from the directory `asset_path`.
    pub(crate) fn new(asset_path: &str) -> Self {
        let root = asset_path.to_owned();
        AssetFileSource { thread: Thread::new("AssetFileSource", move |_| AssetFileSourceThread { root }) }
    }
}

impl FileSource for AssetFileSource {
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let req = FileSourceRequest::new(callback);
        let req_ref = req.actor();
        let resource = resource.clone();
        self.thread.actor().invoke(move |t: &AssetFileSourceThread| t.request(&resource, &req_ref));
        Box::new(req)
    }

    fn can_request(&self, resource: &Resource) -> bool {
        accepts_url(&resource.url)
    }
}
