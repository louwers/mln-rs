use mln_util::constants::FILE_PROTOCOL;
use mln_util::pool::Pool;

use crate::storage::file_source::FileSource;
use crate::storage::local_file_request::local_file_response;
use crate::storage::request::Responder;
use crate::storage::resource::Resource;
use crate::storage::response::{Reason, ResourceError, Response};

fn accepts_url(url: &str) -> bool {
    url.starts_with(FILE_PROTOCOL)
}

/// The percent-decoded path after `file://`. On Android, a query or fragment
/// ends it.
fn path_from_file_url(url: &str) -> Vec<u8> {
    let url = &url[FILE_PROTOCOL.len()..];
    #[cfg(target_os = "android")]
    let url = crate::storage::android::url_path(url);
    #[cfg_attr(not(windows), expect(unused_mut, reason = "only Windows edits the path"))]
    let mut path: Vec<u8> = percent_encoding::percent_decode_str(url).collect();
    #[cfg(windows)]
    {
        // file:///C:/path minus the scheme is /C:/path, which Windows rejects.
        if path.len() >= 3 && path[0] == b'/' && path[2] == b':' && path[1].is_ascii_alphabetic() {
            path.remove(0);
        }
    }
    path
}

/// The file at a `file://` URL, or its error.
fn file_response(resource: &Resource) -> Response {
    if !accepts_url(&resource.url) {
        let mut response = Response::new();
        response.error = Some(Box::new(ResourceError::new(Reason::Other, "Invalid file URL")));
        return response;
    }
    let path = path_from_file_url(&resource.url);
    #[cfg(target_os = "android")]
    if let Some(name) = crate::storage::android::app_asset_name(&path) {
        return crate::storage::android::asset_response(name, resource.data_range);
    }
    local_file_response(&path, resource.data_range)
}

/// The `FileSystem` file source: `file://` URLs, read on a thread pool. On
/// Android, the files under `crate::storage::android::APP_ASSETS` are the app's
/// assets.
pub(crate) struct LocalFileSource {
    pool: Pool,
}

impl LocalFileSource {
    /// Reads files on `pool`.
    #[must_use]
    pub(crate) fn new(pool: Pool) -> Self {
        LocalFileSource { pool }
    }
}

impl FileSource for LocalFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        let resource = resource.clone();
        self.pool.spawn(move || {
            if !responder.is_cancelled() {
                responder.send(file_response(&resource));
            }
        });
    }

    fn can_request(&self, resource: &Resource) -> bool {
        accepts_url(&resource.url)
    }
}
