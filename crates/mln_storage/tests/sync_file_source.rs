// A file source that answers inside `load`, under a `MapCore` that loads a
// style through it. A separate test target because it needs `mln_runtime` and
// `mln_headless`, which depend on `mln_storage`.

use std::sync::Arc;

use mln_headless::HeadlessFrontend;
use mln_runtime::map::{MapCore, MapCoreOptions};
use mln_storage::response::{Reason, ResourceError};
use mln_storage::{FileSource, Resource, Responder, Response};
use mln_util::hash::HashMap;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use mln_util::size::Size;
use parking_lot::Mutex;

fn read_file(relative: &str) -> Vec<u8> {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("Cannot read file {}: {e}", path.display()))
}

#[derive(Default)]
struct SyncFileSource {
    assets: Mutex<HashMap<String, Arc<Vec<u8>>>>,
}

impl SyncFileSource {
    fn add(&self, key: &str, data: Vec<u8>) {
        self.assets.lock().entry(key.to_owned()).or_insert(Arc::new(data));
    }
}

impl FileSource for SyncFileSource {
    fn load(&self, resource: &Resource, responder: Responder) {
        let mut response = Response::new();
        let data = self.assets.lock().get(&resource.url).cloned();
        match data {
            None => {
                response.error =
                    Some(Box::new(ResourceError::new(Reason::NotFound, format!("Not Found: {}", resource.url))));
            }
            Some(data) => response.data = Some(data),
        }
        responder.send(response);
    }

    fn can_request(&self, _resource: &Resource) -> bool {
        true
    }
}

// gtest: SyncFileSource.LoadSyncRender
#[test]
fn load_sync_render() {
    let queue = Queue::new();
    let pool = Pool::new(4);
    let fs = Arc::new(SyncFileSource::default());
    fs.add("maptiler://maps/streets", read_file("test/fixtures/resources/source_vector.json"));
    fs.add("maptiler://sprites/streets/sprite", read_file("test/fixtures/resources/sprite.png"));
    let mut frontend = HeadlessFrontend::new(Size::new(512, 512), 1.0, &queue, &pool).expect("a headless frontend");
    let mut map =
        MapCore::new(fs, MapCoreOptions::default(), Arc::clone(mln_rs::builtin_registry().style()), &pool).unwrap();
    frontend.attach(&mut map);
    let style = String::from_utf8(read_file("test/fixtures/resources/style_vector.json")).unwrap();
    map.load_style_json(&style);
    map.process_events();
}
