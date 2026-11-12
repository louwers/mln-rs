// Ports test/renderer/image_manager.test.cpp.
//
// C++ requestors are stack objects whose destructor calls
// `ImageManager::removeRequestor`; here they are `Rc`s (the manager keeps
// `Weak`s) and `Drop` calls `remove_requestor_key`, which is what the
// renderer's own requestors do.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};

use mln_actor::RunLoop;
use mln_renderer::gfx::dynamic_texture_atlas::ImageVersionMap;
use mln_renderer::image_manager::{ImageManager, ImageManagerObserver, ImageRequestor, ImageRequestorBase};
use mln_style_types::image::{ImageDependencies, ImageMap, ImageType, Impl as ImageImpl};
use mln_util::constants::DEFAULT_ON_DEMAND_IMAGES_CACHE_SIZE;
use mln_util::image::PremultipliedImage;
use mln_util::immutable::Immutable;
use mln_util::logging::{Event, EventSeverity, Log, Observer};
use mln_util::size::Size;

/// `util::read_file("test/fixtures/...")`, from the copy of the C++ files
/// (testdata/).
fn read_file(relative: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

type Messages = Arc<Mutex<Vec<(EventSeverity, Event, i64, String)>>>;

struct Recorder(Messages);

impl Observer for Recorder {
    fn on_record(&self, severity: EventSeverity, event: Event, code: i64, msg: &str) -> bool {
        self.0.lock().unwrap().push((severity, event, code, msg.to_owned()));
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
        let _ = Log::set_thread_observer(Some(Box::new(Recorder(messages.clone()))));
        FixtureLog { messages }
    }

    fn empty(&self) -> bool {
        self.messages.lock().unwrap().is_empty()
    }
}

impl Drop for FixtureLog {
    fn drop(&mut self) {
        let _ = Log::set_thread_observer(None);
    }
}

/// `makeMutable<style::Image::Impl>(id, PremultipliedImage(size), pixelRatio, sdf)`.
fn make_image(id: &str, size: Size, pixel_ratio: f32, sdf: bool) -> Immutable<ImageImpl> {
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

type ImagesAvailable = Box<dyn Fn(ImageMap, ImageMap, ImageVersionMap)>;

struct StubImageRequestor {
    base: ImageRequestorBase,
    image_manager: Rc<ImageManager>,
    images_available: RefCell<Option<ImagesAvailable>>,
    image_correlation_id: Cell<u64>,
    /// The `on_requested_image_added` calls.
    relayouts: Cell<u32>,
}

impl StubImageRequestor {
    fn new(image_manager: &Rc<ImageManager>) -> Rc<StubImageRequestor> {
        Rc::new(StubImageRequestor {
            base: ImageRequestorBase::default(),
            image_manager: image_manager.clone(),
            images_available: RefCell::new(None),
            image_correlation_id: Cell::new(0),
            relayouts: Cell::new(0),
        })
    }
}

impl ImageRequestor for StubImageRequestor {
    fn requestor_base(&self) -> &ImageRequestorBase {
        &self.base
    }

    fn on_requested_image_added(&self) {
        self.relayouts.set(self.relayouts.get() + 1);
    }

    fn on_images_available(
        &self,
        icons: ImageMap,
        patterns: ImageMap,
        version_map: ImageVersionMap,
        image_correlation_id: u64,
    ) {
        if let Some(images_available) = &*self.images_available.borrow()
            && self.image_correlation_id.get() == image_correlation_id
        {
            images_available(icons, patterns, version_map);
        }
    }
}

impl Drop for StubImageRequestor {
    /// `~ImageRequestor()`: `imageManager->removeRequestor(*this)`.
    fn drop(&mut self) {
        self.image_manager.remove_requestor_key(std::ptr::from_ref::<Self>(self).cast::<()>() as usize);
    }
}

/// `imageManager->getImages(requestor, std::make_pair(dependencies, id))`.
fn get_images(
    image_manager: &ImageManager,
    requestor: &Rc<StubImageRequestor>,
    dependencies: ImageDependencies,
    id: u64,
) {
    let requestor: Rc<dyn ImageRequestor> = requestor.clone();
    image_manager.get_images(&requestor, (dependencies, id));
}

/// A plain `ImageManagerObserver` (its default `onStyleImageMissing` calls
/// `done()`).
struct DefaultImageManagerObserver;
impl ImageManagerObserver for DefaultImageManagerObserver {}

type ImageMissing = Box<dyn Fn(&str)>;
type RemoveUnusedStyleImages = Box<dyn Fn(&[String])>;

struct StubImageManagerObserver {
    count: Cell<i32>,
    image_missing: RefCell<ImageMissing>,
    remove_unused_style_images: RefCell<RemoveUnusedStyleImages>,
}

impl StubImageManagerObserver {
    fn new() -> Rc<StubImageManagerObserver> {
        Rc::new(StubImageManagerObserver {
            count: Cell::new(0),
            image_missing: RefCell::new(Box::new(|_| {})),
            remove_unused_style_images: RefCell::new(Box::new(|_| {})),
        })
    }
}

impl ImageManagerObserver for StubImageManagerObserver {
    fn on_style_image_missing(&self, id: &str, done: Box<dyn FnOnce()>) {
        self.count.set(self.count.get() + 1);
        (self.image_missing.borrow())(id);
        done();
    }

    fn on_remove_unused_style_images(&self, unused_image_ids: &[String]) {
        (self.remove_unused_style_images.borrow())(unused_image_ids);
    }
}

// gtest: ImageManager.Missing
#[test]
fn missing() {
    let image_manager = ImageManager::create();
    assert!(image_manager.get_image("doesnotexist").is_none());
}

// gtest: ImageManager.Basic
#[test]
fn basic() {
    let _log = FixtureLog::new();
    let image_manager = ImageManager::create();

    let images = mln_sprite::sprite_parser::parse_sprite(
        "default",
        &read_file("test/fixtures/annotations/emerald.png"),
        &read_file("test/fixtures/annotations/emerald.json"),
    )
    .expect("parseSprite");
    for image in &images {
        image_manager.add_image(image.clone());
        let stored = image_manager.get_image(&image.id);
        let stored = stored.expect("stored");
        assert_eq!(image.image.size, stored.image.size);
    }

    image_manager.dump_debug_logs();
}

// gtest: ImageManager.AddRemove
#[test]
fn add_remove() {
    let _log = FixtureLog::new();
    let image_manager = ImageManager::create();

    image_manager.add_image(make_image("one", Size::new(16, 16), 2.0, false));
    image_manager.add_image(make_image("two", Size::new(16, 16), 2.0, false));
    image_manager.add_image(make_image("three", Size::new(16, 16), 2.0, false));

    image_manager.remove_image("one");
    image_manager.remove_image("two");

    assert!(image_manager.get_image("three").is_some());
    assert!(image_manager.get_image("two").is_none());
    assert!(image_manager.get_image("four").is_none());
}

// gtest: ImageManager.Update
#[test]
fn update() {
    let _log = FixtureLog::new();
    let image_manager = ImageManager::create();

    image_manager.add_image(make_image("one", Size::new(16, 16), 2.0, false));
    assert_eq!(0, image_manager.updated_image_versions().len());
    image_manager.update_image(make_image("one", Size::new(16, 16), 2.0, false));
    assert_eq!(1, image_manager.updated_image_versions().len());
    image_manager.remove_image("one");
    assert_eq!(0, image_manager.updated_image_versions().len());
}

// gtest: ImageManager.SDFChangeRequiresRelayout
#[test]
fn sdf_change_requires_relayout() {
    let _log = FixtureLog::new();
    let image_manager = ImageManager::create();

    image_manager.add_image(make_image("one", Size::new(16, 16), 2.0, false));

    assert!(image_manager.update_image(make_image("one", Size::new(16, 16), 2.0, true)));
    assert_eq!(1, image_manager.updated_image_versions().len());
    let one = image_manager.get_image("one").expect("one");
    assert!(one.sdf);
}

// gtest: ImageManager.RemoveReleasesBinPackRect
#[test]
fn remove_releases_bin_pack_rect() {
    let log = FixtureLog::new();
    let image_manager = ImageManager::create();

    image_manager.add_image(make_image("big", Size::new(32, 32), 1.0, false));
    assert!(image_manager.get_image("big").is_some());

    image_manager.remove_image("big");

    image_manager.add_image(make_image("big", Size::new(32, 32), 1.0, false));
    assert!(image_manager.get_image("big").is_some());
    assert!(log.empty());
}

// gtest: ImageManager.NotifiesRequestorWhenSpriteIsLoaded
#[test]
fn notifies_requestor_when_sprite_is_loaded() {
    let run_loop = RunLoop::new();
    let image_manager = ImageManager::create();
    let requestor = StubImageRequestor::new(&image_manager);
    let notified = Rc::new(Cell::new(false));

    let observer: Rc<dyn ImageManagerObserver> = Rc::new(DefaultImageManagerObserver);
    image_manager.set_observer(Some(observer));

    let n = notified.clone();
    *requestor.images_available.borrow_mut() = Some(Box::new(move |_, _, _| n.set(true)));

    let image_correlation_id = 0;
    get_images(&image_manager, &requestor, dependencies(&["one"]), image_correlation_id);
    run_loop.run_once();

    assert!(!notified.get());

    image_manager.set_loaded(true);
    run_loop.run_once();
    assert!(!notified.get());
    image_manager.notify_if_missing_image_added();
    run_loop.run_once();
    assert!(notified.get());
}

// gtest: ImageManager.NotifiesRequestorImmediatelyIfDependenciesAreSatisfied
#[test]
fn notifies_requestor_immediately_if_dependencies_are_satisfied() {
    let image_manager = ImageManager::create();
    let requestor = StubImageRequestor::new(&image_manager);
    let notified = Rc::new(Cell::new(false));

    let n = notified.clone();
    *requestor.images_available.borrow_mut() = Some(Box::new(move |_, _, _| n.set(true)));

    let image_correlation_id = 0;
    image_manager.add_image(make_image("one", Size::new(16, 16), 2.0, false));
    get_images(&image_manager, &requestor, dependencies(&["one"]), image_correlation_id);

    assert!(notified.get());
}

// gtest: ImageManager.OnStyleImageMissingBeforeSpriteLoaded
#[test]
fn on_style_image_missing_before_sprite_loaded() {
    let run_loop = RunLoop::new();
    let image_manager = ImageManager::create();
    let requestor = StubImageRequestor::new(&image_manager);
    let observer = StubImageManagerObserver::new();

    image_manager.set_observer(Some(observer.clone()));

    let notified = Rc::new(Cell::new(false));

    let n = notified.clone();
    *requestor.images_available.borrow_mut() = Some(Box::new(move |_, _, _| n.set(true)));

    let mut image_correlation_id = 0;
    let mut deps = dependencies(&["pre"]);
    get_images(&image_manager, &requestor, deps.clone(), image_correlation_id);
    run_loop.run_once();

    assert_eq!(observer.count.get(), 0);
    assert!(!notified.get());

    image_manager.set_loaded(true);
    run_loop.run_once();

    assert_eq!(observer.count.get(), 1);
    assert!(!notified.get());

    image_manager.notify_if_missing_image_added();
    run_loop.run_once();

    assert_eq!(observer.count.get(), 1);
    assert!(notified.get());

    // Request for updated dependencies must be dispatched to the
    // observer.
    deps.entry("post".to_owned()).or_insert(ImageType::Icon);
    image_correlation_id += 1;
    get_images(&image_manager, &requestor, deps.clone(), image_correlation_id);
    assert!(requestor.has_pending_requests());

    run_loop.run_once();
    assert!(!requestor.has_pending_requests());

    // Another requestor shall not have pending requests for already obtained images.
    let another_requestor = StubImageRequestor::new(&image_manager);
    image_correlation_id += 1;
    get_images(&image_manager, &another_requestor, deps.clone(), image_correlation_id);
    assert!(!another_requestor.has_pending_requests());

    deps.entry("unfamiliar".to_owned()).or_insert(ImageType::Icon);
    image_correlation_id += 1;
    get_images(&image_manager, &another_requestor, deps.clone(), image_correlation_id);
    assert!(another_requestor.has_pending_requests());
    assert!(another_requestor.has_pending_request("unfamiliar"));
}

// gtest: ImageManager.OnStyleImageMissingAfterSpriteLoaded
#[test]
fn on_style_image_missing_after_sprite_loaded() {
    let run_loop = RunLoop::new();
    let image_manager = ImageManager::create();
    let requestor = StubImageRequestor::new(&image_manager);
    let observer = StubImageManagerObserver::new();

    image_manager.set_observer(Some(observer.clone()));

    let notified = Rc::new(Cell::new(false));

    let n = notified.clone();
    *requestor.images_available.borrow_mut() = Some(Box::new(move |_, _, _| n.set(true)));

    assert_eq!(observer.count.get(), 0);
    assert!(!notified.get());

    image_manager.set_loaded(true);
    run_loop.run_once();

    let image_correlation_id = 0;
    get_images(&image_manager, &requestor, dependencies(&["after"]), image_correlation_id);
    run_loop.run_once();

    assert_eq!(observer.count.get(), 1);
    assert!(!notified.get());

    image_manager.notify_if_missing_image_added();
    run_loop.run_once();

    assert_eq!(observer.count.get(), 1);
    assert!(notified.get());
}

// gtest: ImageManager.RemoveUnusedStyleImages
#[test]
fn remove_unused_style_images() {
    let run_loop = RunLoop::new();
    let image_manager = ImageManager::create();
    let observer = StubImageManagerObserver::new();
    image_manager.set_observer(Some(observer.clone()));
    image_manager.set_loaded(true);

    // The observer is owned by the manager here, so it holds the manager
    // weakly (C++ captures `imageManager` by reference).
    let weak_manager: Weak<ImageManager> = Rc::downgrade(&image_manager);
    *observer.image_missing.borrow_mut() = Box::new(move |id: &str| {
        let image_manager = weak_manager.upgrade().expect("imageManager");
        if id == "1024px" {
            image_manager.add_image(make_image(id, Size::new(1024, 1024), 1.0, false));
        } else {
            image_manager.add_image(make_image(id, Size::new(16, 16), 1.0, false));
        }
    });

    let weak_manager: Weak<ImageManager> = Rc::downgrade(&image_manager);
    *observer.remove_unused_style_images.borrow_mut() = Box::new(move |ids: &[String]| {
        let image_manager = weak_manager.upgrade().expect("imageManager");
        for id in ids {
            assert!(image_manager.get_image(id).is_some());
            image_manager.remove_image(id);
        }
    });

    // Style sprite.
    image_manager.add_image(make_image("sprite", Size::new(16, 16), 1.0, false));

    // Single requestor
    {
        let requestor = StubImageRequestor::new(&image_manager);
        get_images(&image_manager, &requestor, dependencies(&["missing"]), 0);
        run_loop.run_once();
        assert_eq!(observer.count.get(), 1);
        assert!(image_manager.get_image("missing").is_some());
    }

    // Within cache limits, no need to notify client.
    image_manager.reduce_memory_use_if_cache_size_exceeds_limit();
    run_loop.run_once();
    assert!(image_manager.get_image("missing").is_some());

    // Simulate OOM case, forces client notification to be issued.
    image_manager.reduce_memory_use();
    run_loop.run_once();
    assert!(image_manager.get_image("missing").is_none());
    assert!(image_manager.get_image("sprite").is_some());

    // Single requestor, exceed cache size limit.
    {
        let requestor = StubImageRequestor::new(&image_manager);
        get_images(&image_manager, &requestor, dependencies(&["1024px"]), 0);
        run_loop.run_once();
        assert_eq!(observer.count.get(), 2);
        assert!(image_manager.get_image("1024px").is_some());
    }

    // Over cache limits, need to notify client.
    image_manager.reduce_memory_use_if_cache_size_exceeds_limit();
    run_loop.run_once();
    assert!(image_manager.get_image("1024px").is_none());
    assert!(image_manager.get_image("sprite").is_some());

    // Multiple requestors
    {
        let requestor1 = StubImageRequestor::new(&image_manager);
        let requestor2 = StubImageRequestor::new(&image_manager);
        get_images(&image_manager, &requestor1, dependencies(&["missing"]), 0);
        get_images(&image_manager, &requestor2, dependencies(&["missing"]), 1);
        run_loop.run_once();
        assert_eq!(observer.count.get(), 3);
        assert!(image_manager.get_image("missing").is_some());
    }

    // Reduce memory usage and check that unused image was deleted when all requestors are destructed.
    image_manager.reduce_memory_use_if_cache_size_exceeds_limit();
    run_loop.run_once();
    assert!(image_manager.get_image("missing").is_some());
    image_manager.reduce_memory_use();
    run_loop.run_once();
    assert!(image_manager.get_image("missing").is_none());

    // Multiple requestors, check that image resource is not destroyed if there
    // is at least 1 requestor that uses it.
    let mut requestor = Some(StubImageRequestor::new(&image_manager));
    {
        let requestor1 = StubImageRequestor::new(&image_manager);
        get_images(&image_manager, requestor.as_ref().unwrap(), dependencies(&["missing", "1024px"]), 0);
        get_images(&image_manager, &requestor1, dependencies(&["missing"]), 1);
        run_loop.run_once();
        assert_eq!(observer.count.get(), 5);
        assert!(image_manager.get_image("missing").is_some());
        assert!(image_manager.get_image("1024px").is_some());
    }

    // Reduce memory usage and check that requested image is not destructed.
    image_manager.reduce_memory_use_if_cache_size_exceeds_limit();
    run_loop.run_once();
    assert!(image_manager.get_image("missing").is_some());
    assert!(image_manager.get_image("1024px").is_some());

    // Release last requestor and check if resource was released when cache size is over the limit.
    requestor.take();
    image_manager.reduce_memory_use_if_cache_size_exceeds_limit();
    run_loop.run_once();
    assert!(image_manager.get_image("missing").is_none());
    assert!(image_manager.get_image("1024px").is_none());
    assert!(image_manager.get_image("sprite").is_some());
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

    let run_loop = RunLoop::new();
    let image_manager = ImageManager::create();
    let observer = StubImageManagerObserver::new();
    image_manager.set_observer(Some(observer.clone()));
    image_manager.set_loaded(true);

    let weak_manager: Weak<ImageManager> = Rc::downgrade(&image_manager);
    *observer.image_missing.borrow_mut() = Box::new(move |id: &str| {
        let image_manager = weak_manager.upgrade().expect("imageManager");
        image_manager.add_image(make_image(id, Size::new(ON_DEMAND_IMAGE_SIDE, ON_DEMAND_IMAGE_SIDE), 3.0, false));
    });
    let evicted: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let weak_manager: Weak<ImageManager> = Rc::downgrade(&image_manager);
    let evicted_by_observer = evicted.clone();
    *observer.remove_unused_style_images.borrow_mut() = Box::new(move |ids: &[String]| {
        let image_manager = weak_manager.upgrade().expect("imageManager");
        evicted_by_observer.borrow_mut().extend(ids.iter().cloned());
        for id in ids {
            image_manager.remove_image(id);
        }
    });

    let mut requestor = Some(StubImageRequestor::new(&image_manager));
    image_manager.add_image(make_image("sprite", Size::new(16, 16), 1.0, false));
    let dependencies: Rc<RefCell<ImageDependencies>> = Rc::new(RefCell::new(dependencies(&["sprite"])));
    for i in 0..ON_DEMAND_IMAGE_COUNT {
        dependencies.borrow_mut().insert(format!("a{i}"), ImageType::Icon);
    }
    let notified = Rc::new(Cell::new(false));
    {
        let notified = notified.clone();
        let dependencies = dependencies.clone();
        *requestor.as_ref().unwrap().images_available.borrow_mut() =
            Some(Box::new(move |icons: ImageMap, _: ImageMap, _: ImageVersionMap| {
                notified.set(true);
                let dependencies = dependencies.borrow();
                assert_eq!(icons.len(), dependencies.len());
                for id in dependencies.keys() {
                    assert!(icons.contains_key(id), "{id}");
                }
            }));
    }
    let request = |requestor: &Rc<StubImageRequestor>| {
        notified.set(false);
        let correlation_id = requestor.image_correlation_id.get() + 1;
        requestor.image_correlation_id.set(correlation_id);
        get_images(&image_manager, requestor, dependencies.borrow().clone(), correlation_id);
        image_manager.reduce_memory_use_if_cache_size_exceeds_limit();
        assert!(evicted.borrow().is_empty());
        run_loop.run_once();
        image_manager.notify_if_missing_image_added();
        assert!(notified.get());
    };

    request(requestor.as_ref().unwrap());
    assert_eq!(observer.count.get(), 6);
    image_manager.reduce_memory_use_if_cache_size_exceeds_limit();
    assert!(evicted.borrow().is_empty());

    request(requestor.as_ref().unwrap());
    assert_eq!(observer.count.get(), 6);

    for i in 0..ON_DEMAND_IMAGE_COUNT {
        dependencies.borrow_mut().insert(format!("b{i}"), ImageType::Icon);
    }
    for _round in 0..2 {
        request(requestor.as_ref().unwrap());
        image_manager.reduce_memory_use_if_cache_size_exceeds_limit();
        assert!(evicted.borrow().is_empty());
        assert_eq!(observer.count.get(), 12);
        for id in dependencies.borrow().keys() {
            assert!(image_manager.get_image(id).is_some(), "{id}");
        }
    }

    requestor.take();
    image_manager.reduce_memory_use_if_cache_size_exceeds_limit();
    assert_eq!(evicted.borrow().len(), 12);
    dependencies.borrow_mut().remove("sprite");
    for id in dependencies.borrow().keys() {
        assert!(image_manager.get_image(id).is_none(), "{id}");
    }
    assert!(image_manager.get_image("sprite").is_some());
}

// Port-only: maplibre-native-ffi's late style image layout (its patch 0030).
// A requestor answered without an image it asked for relayouts once the
// image is added, after all of the update's images are; one that didn't ask
// for it doesn't.
#[test]
fn requestor_relayouts_when_requested_image_is_added() {
    let run_loop = RunLoop::new();
    let image_manager = ImageManager::create();
    image_manager.set_observer(Some(Rc::new(DefaultImageManagerObserver)));
    image_manager.set_loaded(true);
    let requestor = StubImageRequestor::new(&image_manager);
    let unrelated = StubImageRequestor::new(&image_manager);
    let answered = Rc::new(Cell::new(0));
    for stub in [&requestor, &unrelated] {
        let answered = answered.clone();
        *stub.images_available.borrow_mut() = Some(Box::new(move |_, _, _| answered.set(answered.get() + 1)));
    }

    get_images(&image_manager, &requestor, dependencies(&["marker"]), 0);
    get_images(&image_manager, &unrelated, dependencies(&["other"]), 0);
    run_loop.run_once();
    image_manager.notify_if_missing_image_added();
    assert_eq!(answered.get(), 2);

    image_manager.add_image(make_image("marker", Size::new(16, 16), 1.0, false));
    image_manager.add_image(make_image("unrequested", Size::new(16, 16), 1.0, false));
    assert_eq!(requestor.relayouts.get(), 0);
    image_manager.notify_if_missing_image_added();
    assert_eq!(requestor.relayouts.get(), 1);
    assert_eq!(unrelated.relayouts.get(), 0);

    image_manager.notify_if_missing_image_added();
    assert_eq!(requestor.relayouts.get(), 1);
}

// Port-only: as above. A requestor still waiting for its answer gets an
// image added in the meantime with the answer, without a relayout.
#[test]
fn pending_requestor_gets_added_image_without_relayout() {
    let run_loop = RunLoop::new();
    let image_manager = ImageManager::create();
    image_manager.set_observer(Some(Rc::new(DefaultImageManagerObserver)));
    image_manager.set_loaded(true);
    let requestor = StubImageRequestor::new(&image_manager);
    let icons = Rc::new(RefCell::new(Vec::new()));
    {
        let icons = icons.clone();
        *requestor.images_available.borrow_mut() =
            Some(Box::new(move |received: ImageMap, _, _| icons.borrow_mut().extend(received.keys().cloned())));
    }

    get_images(&image_manager, &requestor, dependencies(&["marker"]), 0);
    image_manager.add_image(make_image("marker", Size::new(16, 16), 1.0, false));
    run_loop.run_once();
    image_manager.notify_if_missing_image_added();

    assert_eq!(*icons.borrow(), ["marker"]);
    assert_eq!(requestor.relayouts.get(), 0);
}

// Port-only: as above. A requestor that goes away before the update's
// images are all added isn't told.
#[test]
fn removed_requestor_does_not_relayout() {
    let run_loop = RunLoop::new();
    let image_manager = ImageManager::create();
    image_manager.set_observer(Some(Rc::new(DefaultImageManagerObserver)));
    image_manager.set_loaded(true);
    let requestor = StubImageRequestor::new(&image_manager);
    get_images(&image_manager, &requestor, dependencies(&["marker"]), 0);
    run_loop.run_once();
    image_manager.notify_if_missing_image_added();

    image_manager.add_image(make_image("marker", Size::new(16, 16), 1.0, false));
    image_manager.remove_requestor_key(Rc::as_ptr(&requestor).cast::<()>() as usize);
    image_manager.notify_if_missing_image_added();
    assert_eq!(requestor.relayouts.get(), 0);
}
