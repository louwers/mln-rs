// A file source that answers inside `load`, under a `Map` that loads a
// style through it.

use std::sync::Arc;

use mln_headless::HeadlessFrontend;
use mln_map::{Map, MapOptions};
use mln_storage::response::{Reason, ResourceError};
use mln_storage::{FileSource, Resource, Responder, Response};
use mln_style::hash::HashMap;
use mln_style::size::Size;
use mln_util::pool::Pool;
use mln_util::queue::Queue;
use parking_lot::Mutex;

use crate::support::{read_bytes, read_file};

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
    fs.add("maptiler://maps/streets", read_bytes("test/fixtures/resources/source_vector.json"));
    fs.add("maptiler://sprites/streets/sprite", read_bytes("test/fixtures/resources/sprite.png"));
    let mut frontend = HeadlessFrontend::new(Size::new(512, 512), 1.0, &queue, &pool).expect("a headless frontend");
    let mut map = Map::with_file_source(fs, MapOptions::default(), &pool).unwrap();
    frontend.attach(&mut map);
    let style = read_file("test/fixtures/resources/style_vector.json");
    map.load_style_json(&style);
    map.process_events();
}
