use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use parking_lot::Mutex;

use super::{LocalRequests, TestLoop, cpp_path, req};
use crate::storage::asset_file_source::AssetFileSource;
use crate::storage::file_source::{FileSource, FileSourceExt};
use crate::storage::request::Request;
use crate::storage::resource::{Kind, Resource};
use crate::storage::response::{Reason, Response};

fn asset_file_source() -> AssetFileSource {
    AssetFileSource::new(cpp_path("test/fixtures/storage/assets").to_str().unwrap(), super::pool())
}

/// Requests `resource`, runs the loop until the response and returns it.
fn request(fs: &AssetFileSource, resource: &Resource) -> Response {
    let run_loop = TestLoop::new();
    let local = run_loop.dispatcher();
    let req = req();
    let result = Rc::new(RefCell::new(None));
    let (r, out, stop) = (Rc::clone(&req), Rc::clone(&result), run_loop.clone());
    *req.borrow_mut() = Some(fs.request_local(
        &local,
        resource,
        Box::new(move |res: Response| {
            r.borrow_mut().take();
            *out.borrow_mut() = Some(res);
            stop.stop();
        }),
    ));
    run_loop.run();

    result.borrow_mut().take().expect("response")
}

fn data(res: &Response) -> &[u8] {
    res.data.as_deref().map(Vec::as_slice).expect("data")
}

/// A chain of requests for an asset, each made by the callback of the one
/// before.
struct WorkerChain {
    fs: Arc<AssetFileSource>,
    num_requests: AtomicU32,
    request: Mutex<Option<Request>>,
    end_callback: Box<dyn Fn() + Send + Sync>,
    failures: Arc<Mutex<Vec<String>>>,
}

impl WorkerChain {
    fn request(self: &Arc<Self>) {
        let asset = "asset://nonempty";
        let this = Arc::clone(self);
        // Held while the request starts, so that its callback's next
        // request takes its place only after it is stored.
        let mut slot = self.request.lock();
        *slot = Some(self.fs.request(&Resource::new(Kind::Unknown, asset), move |res: Response| {
            // EXPECT_*: record, keep going.
            if res.error.is_some() || res.data.as_deref().map(Vec::as_slice) != Some(b"content is here\n") {
                this.failures.lock().push(format!("{res:?}"));
            }

            if this.num_requests.fetch_sub(1, Ordering::SeqCst) == 1 {
                (this.end_callback)();
                drop(this.request.lock().take());
            } else {
                this.request();
            }
        }));
    }
}

// gtest: AssetFileSource.Load
#[test]
fn load() {
    let run_loop = TestLoop::new();

    let fs = Arc::new(asset_file_source());

    let num_threads = 50;
    let completed = Arc::new(AtomicUsize::new(num_threads));
    let failures = Arc::new(Mutex::new(Vec::new()));

    let mut threads = Vec::new();

    for _ in 0..num_threads {
        let (completed, stop) = (Arc::clone(&completed), run_loop.handle());
        let chain = Arc::new(WorkerChain {
            fs: Arc::clone(&fs),
            num_requests: AtomicU32::new(1000),
            request: Mutex::new(None),
            end_callback: Box::new(move || {
                if completed.fetch_sub(1, Ordering::SeqCst) == 1 {
                    stop.stop();
                }
            }),
            failures: Arc::clone(&failures),
        });
        // Each chain starts on a thread of its own, and goes on on the
        // threads that answer it.
        threads.push(std::thread::spawn(move || chain.request()));
    }

    for thread in threads {
        thread.join().unwrap();
    }
    run_loop.run();
    assert!(failures.lock().is_empty(), "{:?}", failures.lock().first());
}

// gtest: AssetFileSource.AcceptsURL
#[test]
fn accepts_url() {
    let fs = asset_file_source();
    assert!(fs.can_request(&Resource::style("asset://empty")));
    assert!(fs.can_request(&Resource::style("asset:///test")));
    assert!(!fs.can_request(&Resource::style("assds://foo")));
    assert!(!fs.can_request(&Resource::style("asset:")));
    assert!(!fs.can_request(&Resource::style("style.json")));
    assert!(!fs.can_request(&Resource::style("")));
}

// gtest: AssetFileSource.EmptyFile
#[test]
fn empty_file() {
    let fs = asset_file_source();
    let res = request(&fs, &Resource::new(Kind::Unknown, "asset://empty"));
    assert_eq!(None, res.error);
    assert_eq!(b"", data(&res));
}

// gtest: AssetFileSource.NonEmptyFile
#[test]
fn non_empty_file() {
    let fs = asset_file_source();
    let res = request(&fs, &Resource::new(Kind::Unknown, "asset://nonempty"));
    assert_eq!(None, res.error);
    assert_eq!(b"content is here\n", data(&res));
}

// gtest: AssetFileSource.NonExistentFile
#[test]
fn non_existent_file() {
    let fs = asset_file_source();
    let res = request(&fs, &Resource::new(Kind::Unknown, "asset://does_not_exist"));
    let error = res.error.as_ref().expect("error");
    assert_eq!(Reason::NotFound, error.reason);
    assert!(res.data.is_none());
    // Do not assert on platform-specific error message.
}

// gtest: AssetFileSource.InvalidURL
#[test]
fn invalid_url() {
    let fs = asset_file_source();
    let res = request(&fs, &Resource::new(Kind::Unknown, "test://wrong-scheme"));
    let error = res.error.as_ref().expect("error");
    assert_eq!(Reason::Other, error.reason);
    assert_eq!("Invalid asset URL", error.message);
    assert!(res.data.is_none());
}

// gtest: AssetFileSource.ReadDirectory
#[test]
fn read_directory() {
    let fs = asset_file_source();
    let res = request(&fs, &Resource::new(Kind::Unknown, "asset://directory"));
    let error = res.error.as_ref().expect("error");
    assert_eq!(Reason::NotFound, error.reason);
    assert!(res.data.is_none());
    // Do not assert on platform-specific error message.
}

// gtest: AssetFileSource.URLEncoding
#[test]
fn url_encoding() {
    let fs = asset_file_source();
    let res = request(&fs, &Resource::new(Kind::Unknown, "asset://%6eonempty"));
    assert_eq!(None, res.error);
    assert_eq!(b"content is here\n", data(&res));
}

// gtest: AssetFileSource.RangeRequest
#[test]
fn range_request() {
    let fs = asset_file_source();

    let mut resource = Resource::new(Kind::Unknown, "asset://nonempty");
    resource.data_range = Some((4, 12));

    let res = request(&fs, &resource);
    assert_eq!(None, res.error);
    assert_eq!(b"ent is he", data(&res));
}
