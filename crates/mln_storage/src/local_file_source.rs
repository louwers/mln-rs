// The requests run on the "LocalFileSource" thread (`util::Thread`) as in
// C++. The resource options are kept next to the thread behind a
// mutex (C++ keeps them in the thread object and reads them back with a
// blocking `ask`); the values returned are the same.

use mln_actor::{ActorRef, Thread};
use mln_util::constants::FILE_PROTOCOL;
use mln_util::url::percent_decode;
use parking_lot::Mutex;

use crate::async_request::AsyncRequest;
use crate::file_source::{Callback, FileSource};
use crate::file_source_request::{FileSourceRequest, FileSourceRequestState};
use crate::local_file_request::request_local_file;
use crate::resource::Resource;
use crate::resource_options::ResourceOptions;
use crate::response::{Reason, ResourceError, Response};

fn accepts_url(url: &str) -> bool {
    url.starts_with(FILE_PROTOCOL)
}

/// The percent-decoded path after `file://`. On Android, a query or fragment
/// ends it.
fn path_from_file_url(url: &str) -> Vec<u8> {
    let url = &url[FILE_PROTOCOL.len()..];
    #[cfg(target_os = "android")]
    let url = crate::android::url_path(url);
    #[allow(unused_mut)]
    let mut path = percent_decode(url);
    #[cfg(windows)]
    {
        // file:///C:/path minus the scheme is /C:/path, which Windows rejects.
        if path.len() >= 3 && path[0] == b'/' && path[2] == b':' && path[1].is_ascii_alphabetic() {
            path.remove(0);
        }
    }
    path
}

pub(crate) struct LocalFileSourceThread;

impl LocalFileSourceThread {
    #[expect(clippy::unused_self, reason = "the file source's actor calls it on its thread's object")]
    pub(crate) fn request(&self, resource: &Resource, req: &ActorRef<FileSourceRequestState>) {
        if !accepts_url(&resource.url) {
            let mut response = Response::new();
            response.error = Some(Box::new(ResourceError::new(Reason::Other, "Invalid file URL")));
            req.invoke(move |request: &FileSourceRequestState| request.set_response(response));
            return;
        }

        let path = path_from_file_url(&resource.url);
        #[cfg(target_os = "android")]
        if let Some(name) = crate::android::app_asset_name(&path) {
            crate::android::request_asset(name, req, resource.data_range);
            return;
        }
        request_local_file(&path, req, resource.data_range);
    }
}

/// The `FileSystem` file source: `file://` URLs. On Android, the files under
/// `mln_storage::android::APP_ASSETS` are the app's assets.
pub(crate) struct LocalFileSource {
    thread: Thread<LocalFileSourceThread>,
    resource_options: Mutex<ResourceOptions>,
}

impl LocalFileSource {
    #[must_use]
    pub(crate) fn new(resource_options: &ResourceOptions) -> Self {
        LocalFileSource {
            thread: Thread::new("LocalFileSource", |_| LocalFileSourceThread),
            resource_options: Mutex::new(resource_options.clone()),
        }
    }
}

impl FileSource for LocalFileSource {
    fn request(&self, resource: &Resource, callback: Callback) -> Box<dyn AsyncRequest> {
        let req = FileSourceRequest::new(callback);
        let req_ref = req.actor();
        let resource = resource.clone();
        self.thread.actor().invoke(move |t: &LocalFileSourceThread| t.request(&resource, &req_ref));
        Box::new(req)
    }

    fn can_request(&self, resource: &Resource) -> bool {
        accepts_url(&resource.url)
    }

    fn set_resource_options(&self, options: ResourceOptions) {
        *self.resource_options.lock() = options;
    }

    fn get_resource_options(&self) -> ResourceOptions {
        self.resource_options.lock().clone()
    }
}
