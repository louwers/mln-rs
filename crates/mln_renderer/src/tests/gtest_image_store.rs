// The C++ ImageManager notifies requestors; the port's ImageStore answers
// waits that ask it again (`resolve`), so a C++ requestor is an
// `ImageWait` here, asked again after each change.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_style_types::image::{ImageDependencies, ImageType, Impl as ImageImpl};
use mln_util::constants::DEFAULT_ON_DEMAND_IMAGES_CACHE_SIZE;
use mln_util::hash::HashSet;
use mln_util::image::PremultipliedImage;
use mln_util::logging::{Event, EventSeverity, Log, Observer};
use mln_util::size::Size;
use parking_lot::Mutex;

use crate::image_store::{ImageStore, ImageStoreObserver, ImageWait};

/// Reads a test fixture from the copy of the C++ files (testdata/).
fn read_file(relative: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

type Messages = Arc<Mutex<Vec<(EventSeverity, Event, i64, String)>>>;

struct Recorder(Messages);

impl Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> bool {
        self.0.lock().push((severity, event, code, msg.to_owned()));
        true
    }
}

/// `test/src/mln/test/fixture_log_observer.hpp` `FixtureLog` (for the calling
/// thread).
struct FixtureLog {
    messages: Messages,
}

impl FixtureLog {
    fn new() -> Self {
        let messages: Messages = Arc::new(Mutex::new(Vec::new()));
        let _ = Log::set_thread_observer(Some(Box::new(Recorder(Arc::clone(&messages)))));
        FixtureLog { messages }
    }

    fn empty(&self) -> bool {
        self.messages.lock().is_empty()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        let _ = Log::set_thread_observer(None);
    }
}

fn make_image(id: &str, size: Size, pixel_ratio: f32, sdf: bool) -> Arc<ImageImpl> {
    Arc::new(
        ImageImpl::new(
            id.to_owned(),
            PremultipliedImage::new(size),
            pixel_ratio,
            sdf,
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
        )
        .expect("image"),
    )
}

fn dependencies(ids: &[&str]) -> ImageDependencies {
    ids.iter().map(|id| (id.to_string(), ImageType::Icon)).collect()
}

/// Records the images asked for and the images found unused.
#[derive(Default)]
struct StubImageStoreObserver {
    missing: RefCell<Vec<String>>,
    unused: RefCell<Vec<String>>,
}

impl ImageStoreObserver for StubImageStoreObserver {
    fn on_style_image_missing(&self, id: &str) {
        self.missing.borrow_mut().push(id.to_owned());
    }

    fn on_remove_unused_style_images(&self, unused_image_ids: &[String]) {
        self.unused.borrow_mut().extend(unused_image_ids.iter().cloned());
    }
}

fn store_with_observer() -> (ImageStore, Rc<StubImageStoreObserver>) {
    let mut store = ImageStore::default();
    let observer = Rc::new(StubImageStoreObserver::default());
    store.set_observer(Some(Rc::<StubImageStoreObserver>::clone(&observer)));
    (store, observer)
}

/// The icons a resolve answered with, sorted.
fn icons(store: &mut ImageStore, wants: &ImageDependencies, wait: &mut ImageWait) -> Option<Vec<String>> {
    let resolved = store.resolve(wants, wait)?;
    Some(resolved.icons.keys().cloned().collect())
}

// gtest: ImageManager.Missing
#[test]
fn missing() {
    let store = ImageStore::default();
    assert!(store.get_image("doesnotexist").is_none());
}

// gtest: ImageManager.Basic
#[test]
fn basic() {
    let _log = FixtureLog::new();
    let mut store = ImageStore::default();

    let images = mln_sprite::sprite_parser::parse_sprite(
        "default",
        &read_file("test/fixtures/annotations/emerald.png"),
        &read_file("test/fixtures/annotations/emerald.json"),
    )
    .expect("parseSprite");
    for image in &images {
        store.add_image(Arc::clone(image));
        let stored = store.get_image(&image.id).expect("stored");
        assert_eq!(image.image.size, stored.image.size);
    }

    store.dump_debug_logs();
}

// gtest: ImageManager.AddRemove
#[test]
fn add_remove() {
    let _log = FixtureLog::new();
    let mut store = ImageStore::default();

    store.add_image(make_image("one", Size::new(16, 16), 2.0, false));
    store.add_image(make_image("two", Size::new(16, 16), 2.0, false));
    store.add_image(make_image("three", Size::new(16, 16), 2.0, false));

    store.remove_image("one");
    store.remove_image("two");

    assert!(store.get_image("three").is_some());
    assert!(store.get_image("two").is_none());
    assert!(store.get_image("four").is_none());
    assert_eq!(store.available_images().iter().collect::<Vec<_>>(), ["three"]);
}

// gtest: ImageManager.Update
#[test]
fn update() {
    let _log = FixtureLog::new();
    let mut store = ImageStore::default();

    store.add_image(make_image("one", Size::new(16, 16), 2.0, false));
    assert!(!store.update_image(make_image("one", Size::new(16, 16), 2.0, false)), "the atlas patches it");
    assert!(store.update_image(make_image("one", Size::new(32, 16), 2.0, false)), "a new size lays out again");
}

// gtest: ImageManager.SDFChangeRequiresRelayout
#[test]
fn sdf_change_requires_relayout() {
    let _log = FixtureLog::new();
    let mut store = ImageStore::default();

    store.add_image(make_image("one", Size::new(16, 16), 2.0, false));

    assert!(store.update_image(make_image("one", Size::new(16, 16), 2.0, true)));
    assert!(store.get_image("one").expect("one").sdf);
}

// gtest: ImageManager.RemoveReleasesBinPackRect
#[test]
fn remove_releases_bin_pack_rect() {
    let log = FixtureLog::new();
    let mut store = ImageStore::default();

    store.add_image(make_image("big", Size::new(32, 32), 1.0, false));
    assert!(store.get_image("big").is_some());

    store.remove_image("big");

    store.add_image(make_image("big", Size::new(32, 32), 1.0, false));
    assert!(store.get_image("big").is_some());
    assert!(log.empty());
}

// gtest: ImageManager.NotifiesRequestorWhenSpriteIsLoaded
#[test]
fn notifies_requestor_when_sprite_is_loaded() {
    let mut store = ImageStore::default();
    let mut wait = ImageWait::default();
    let wants = dependencies(&["one"]);

    store.begin_update();
    assert!(store.resolve(&wants, &mut wait).is_none(), "the sprite may still bring it");

    let revision = store.revision();
    store.set_loaded(true);
    assert_ne!(store.revision(), revision, "a waiting tile asks again");
    assert!(store.resolve(&wants, &mut wait).is_none(), "the app gets an update to add it");
    store.begin_update();
    assert_eq!(icons(&mut store, &wants, &mut wait), Some(Vec::new()));
}

// gtest: ImageManager.NotifiesRequestorImmediatelyIfDependenciesAreSatisfied
#[test]
fn notifies_requestor_immediately_if_dependencies_are_satisfied() {
    let mut store = ImageStore::default();
    store.add_image(make_image("one", Size::new(16, 16), 2.0, false));
    let resolved = store.resolve(&dependencies(&["one"]), &mut ImageWait::default());
    assert_eq!(resolved.map(|resolved| resolved.icons.len()), Some(1));
}

// gtest: ImageManager.OnStyleImageMissingBeforeSpriteLoaded
#[test]
fn on_style_image_missing_before_sprite_loaded() {
    let (mut store, observer) = store_with_observer();
    let mut wait = ImageWait::default();
    let mut wants = dependencies(&["pre"]);

    store.begin_update();
    assert!(store.resolve(&wants, &mut wait).is_none());
    assert!(observer.missing.borrow().is_empty());

    store.set_loaded(true);
    assert!(store.resolve(&wants, &mut wait).is_none());
    assert_eq!(*observer.missing.borrow(), ["pre"]);

    store.begin_update();
    assert_eq!(icons(&mut store, &wants, &mut wait), Some(Vec::new()));
    assert_eq!(*observer.missing.borrow(), ["pre"]);

    // A wait for updated dependencies asks the observer again.
    wants.insert("post".to_owned(), ImageType::Icon);
    let mut wait = ImageWait::default();
    assert!(store.resolve(&wants, &mut wait).is_none());
    assert_eq!(*observer.missing.borrow(), ["pre", "post", "pre"]);

    // Another wait in the same update doesn't ask for them again, and waits
    // as long.
    let mut another = ImageWait::default();
    assert!(store.resolve(&wants, &mut another).is_none());
    assert_eq!(observer.missing.borrow().len(), 3);
    store.begin_update();
    assert!(store.resolve(&wants, &mut another).is_some());
}

// gtest: ImageManager.OnStyleImageMissingAfterSpriteLoaded
#[test]
fn on_style_image_missing_after_sprite_loaded() {
    let (mut store, observer) = store_with_observer();
    store.set_loaded(true);
    store.begin_update();

    let mut wait = ImageWait::default();
    let wants = dependencies(&["after"]);
    assert!(store.resolve(&wants, &mut wait).is_none());
    assert_eq!(*observer.missing.borrow(), ["after"]);

    store.begin_update();
    assert_eq!(icons(&mut store, &wants, &mut wait), Some(Vec::new()));
    assert_eq!(observer.missing.borrow().len(), 1);
}

/// The app adds the image it is asked for before the next update: the wait
/// resolves with it.
#[test]
fn an_image_added_for_a_missing_image_event_resolves_the_wait() {
    let (mut store, observer) = store_with_observer();
    store.set_loaded(true);
    store.begin_update();

    let mut wait = ImageWait::default();
    let wants = dependencies(&["late"]);
    assert!(store.resolve(&wants, &mut wait).is_none());
    assert_eq!(*observer.missing.borrow(), ["late"]);

    store.add_image(make_image("late", Size::new(16, 16), 1.0, false));
    store.begin_update();
    assert_eq!(icons(&mut store, &wants, &mut wait), Some(vec!["late".to_owned()]));
}

// gtest: ImageManager.RemoveUnusedStyleImages
#[test]
fn remove_unused_style_images() {
    let (mut store, observer) = store_with_observer();
    store.set_loaded(true);
    store.add_image(make_image("sprite", Size::new(16, 16), 1.0, false));

    // Asks for the images, which the app adds.
    let request = |store: &mut ImageStore, ids: &[&str], size: u32| {
        store.begin_update();
        assert!(store.resolve(&dependencies(ids), &mut ImageWait::default()).is_none());
        for id in ids {
            if store.get_image(id).is_none() {
                store.add_image(make_image(id, Size::new(size, size), 1.0, false));
            }
        }
    };
    let unused = |store: &ImageStore, used: &[&str]| {
        observer.unused.borrow_mut().clear();
        let used: HashSet<String> = used.iter().map(|id| (*id).to_owned()).collect();
        store.reduce_memory_use(&used);
        let mut unused = observer.unused.borrow().clone();
        unused.sort();
        unused
    };

    request(&mut store, &["missing"], 16);
    assert!(!store.exceeds_cache_size_limit());
    assert_eq!(unused(&store, &["missing"]), Vec::<String>::new());
    assert_eq!(unused(&store, &[]), ["missing"], "the sprite's images aren't requested ones");
    store.remove_image("missing");

    request(&mut store, &["1024px"], 1024);
    assert!(store.exceeds_cache_size_limit());
    assert_eq!(unused(&store, &[]), ["1024px"]);
    store.remove_image("1024px");
    assert!(!store.exceeds_cache_size_limit());

    request(&mut store, &["missing", "1024px"], 16);
    assert_eq!(unused(&store, &["missing", "1024px"]), Vec::<String>::new());
    assert_eq!(unused(&store, &["missing"]), ["1024px"]);
}

// gtest: ImageManager.MixedDependenciesRetainProvidedImages
#[test]
fn mixed_dependencies_retain_provided_images() {
    const ON_DEMAND_IMAGE_COUNT: usize = 6;
    const ON_DEMAND_IMAGE_SIDE: u32 = 256;
    const _: () = assert!(
        ON_DEMAND_IMAGE_COUNT * (ON_DEMAND_IMAGE_SIDE * ON_DEMAND_IMAGE_SIDE * 4) as usize
            > DEFAULT_ON_DEMAND_IMAGES_CACHE_SIZE
    );

    let (mut store, observer) = store_with_observer();
    store.set_loaded(true);
    store.add_image(make_image("sprite", Size::new(16, 16), 1.0, false));
    let mut wants = dependencies(&["sprite"]);
    for i in 0..ON_DEMAND_IMAGE_COUNT {
        wants.insert(format!("a{i}"), ImageType::Icon);
    }

    // One wait: asks for the missing images, which the app adds, and
    // resolves with all of them in the next update.
    let request = |store: &mut ImageStore, wants: &ImageDependencies| {
        store.begin_update();
        let mut wait = ImageWait::default();
        if store.resolve(wants, &mut wait).is_none() {
            for id in observer.missing.borrow().iter() {
                if store.get_image(id).is_none() {
                    store.add_image(make_image(id, Size::new(ON_DEMAND_IMAGE_SIDE, ON_DEMAND_IMAGE_SIDE), 3.0, false));
                }
            }
            store.begin_update();
        }
        let icons = icons(store, wants, &mut wait).expect("resolved");
        assert_eq!(icons.len(), wants.len());
    };
    let used = |wants: &ImageDependencies| wants.keys().cloned().collect::<HashSet<String>>();

    request(&mut store, &wants);
    assert_eq!(observer.missing.borrow().len(), 6);
    store.reduce_memory_use(&used(&wants));
    assert!(observer.unused.borrow().is_empty());

    request(&mut store, &wants);
    assert_eq!(observer.missing.borrow().len(), 6);

    for i in 0..ON_DEMAND_IMAGE_COUNT {
        wants.insert(format!("b{i}"), ImageType::Icon);
    }
    for _round in 0..2 {
        request(&mut store, &wants);
        assert!(store.exceeds_cache_size_limit());
        store.reduce_memory_use(&used(&wants));
        assert!(observer.unused.borrow().is_empty());
        assert_eq!(observer.missing.borrow().len(), 12);
    }

    // No tile uses them any more.
    store.reduce_memory_use(&HashSet::default());
    assert_eq!(observer.unused.borrow().len(), 12);
    assert!(!observer.unused.borrow().contains(&"sprite".to_owned()));
}

/// Removing an image and adding one change the store's revision, so the
/// waiting tiles resolve again.
#[test]
fn changes_bump_the_revision() {
    let mut store = ImageStore::default();
    let revision = store.revision();
    store.add_image(make_image("one", Size::new(16, 16), 1.0, false));
    let added = store.revision();
    assert_ne!(added, revision);
    store.remove_image("one");
    assert_ne!(store.revision(), added);

    // An update without asks changes nothing.
    let quiet = store.revision();
    store.begin_update();
    assert_eq!(store.revision(), quiet);
}
