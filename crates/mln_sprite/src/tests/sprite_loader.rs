// Ports test/sprite/sprite_loader.test.cpp.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::{RunLoop, TaggedScheduler, scheduler};
use mln_storage::response::{Error, Reason};
use mln_storage::stub_file_source::{ResponseType, StubFileSource};
use mln_storage::{Kind, Resource, Response};
use mln_style_types::image::Impl;
use mln_style_types::sprite::Sprite;
use mln_util::exception::ExceptionPtr;
use mln_util::identity::SimpleIdentity;
use mln_util::immutable::Immutable;

use super::{FixtureLog, cpp_path};
use crate::sprite_loader::SpriteLoader;
use crate::sprite_loader_observer::SpriteLoaderObserver;

type Loaded = Box<dyn FnMut(Vec<Immutable<Impl>>)>;
type Failed = Box<dyn FnMut(ExceptionPtr)>;

#[derive(Default)]
struct StubSpriteLoaderObserver {
    sprite_loaded: RefCell<Option<Loaded>>,
    sprite_error: RefCell<Option<Failed>>,
}

impl SpriteLoaderObserver for StubSpriteLoaderObserver {
    fn on_sprite_loaded(&self, _sprite: Option<Sprite>, images: Vec<Immutable<Impl>>) {
        if let Some(f) = self.sprite_loaded.borrow_mut().as_mut() {
            f(images);
        }
    }

    fn on_sprite_error(&self, _sprite: Option<Sprite>, error: ExceptionPtr) {
        if let Some(f) = self.sprite_error.borrow_mut().as_mut() {
            f(error);
        }
    }
}

fn default_sprite_path() -> String {
    cpp_path("test/fixtures/resources/sprite").to_string_lossy().into_owned()
}

struct SpriteLoaderTest {
    run_loop: RunLoop,
    file_source: StubFileSource,
    observer: Rc<StubSpriteLoaderObserver>,
    thread_pool: TaggedScheduler,
    sprite_loader: SpriteLoader,
    sprite_path: String,
    _log: FixtureLog,
}

impl SpriteLoaderTest {
    fn new(pixel_ratio: f32) -> Self {
        let run_loop = RunLoop::new();
        let thread_pool = TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new());
        SpriteLoaderTest {
            run_loop,
            file_source: StubFileSource::new(ResponseType::Asynchronous),
            observer: Rc::new(StubSpriteLoaderObserver::default()),
            sprite_loader: SpriteLoader::new(pixel_ratio, thread_pool.clone()),
            thread_pool,
            sprite_path: default_sprite_path(),
            // Squelch logging.
            _log: FixtureLog::new(),
        }
    }

    fn run(&self) {
        self.sprite_loader.set_observer(self.observer.clone());

        let sprite = Sprite::new("default", self.sprite_path.clone());
        self.sprite_loader.load(Some(sprite), &self.file_source);

        self.run_loop.run();
    }

    fn end(&self) {
        self.run_loop.stop();
    }
}

impl Drop for SpriteLoaderTest {
    fn drop(&mut self) {
        self.thread_pool.wait_for_empty();
    }
}

fn successful_response(
    extension: &'static str,
    sprite_path: String,
    pixel_ratio: f32,
) -> impl Fn(&Resource) -> Option<Response> {
    move |resource: &Resource| {
        assert_eq!(format!("{sprite_path}{}{extension}", if pixel_ratio == 2.0 { "@2x" } else { "" }), resource.url);
        let mut response = Response::new();
        response.data = Some(Arc::new(std::fs::read(&resource.url).unwrap()));
        Some(response)
    }
}

fn failed_sprite_response(_: &Resource) -> Option<Response> {
    let mut response = Response::new();
    response.error = Some(Box::new(Error::new(Reason::Other, "Failed by the test case")));
    Some(response)
}

fn corrupt_sprite_response(_: &Resource) -> Option<Response> {
    let mut response = Response::new();
    response.data = Some(Arc::new(b"CORRUPT".to_vec()));
    Some(response)
}

fn loading_success(sprite_path: String, expected_images: usize, pixel_ratio: f32) {
    let mut test = SpriteLoaderTest::new(pixel_ratio);
    test.sprite_path = sprite_path.clone();

    test.file_source.set_response(Kind::SpriteImage, successful_response(".png", sprite_path.clone(), pixel_ratio));
    test.file_source.set_response(Kind::SpriteJSON, successful_response(".json", sprite_path, pixel_ratio));

    let failure: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let count: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));
    let (f, stop) = (failure.clone(), test.run_loop.clone());
    *test.observer.sprite_error.borrow_mut() = Some(Box::new(move |error| {
        *f.borrow_mut() = Some(error.to_string());
        stop.stop();
    }));
    let (c, stop) = (count.clone(), test.run_loop.clone());
    *test.observer.sprite_loaded.borrow_mut() = Some(Box::new(move |images| {
        *c.borrow_mut() = Some(images.len());
        stop.stop();
    }));

    test.run();
    assert_eq!(None, *failure.borrow());
    assert_eq!(Some(expected_images), *count.borrow());
}

// INSTANTIATE_TEST_SUITE_P(SpriteLoaderSprites, SpriteLoaderParametrized, ...)
// gtest: SpriteLoaderParametrized.LoadingSuccess
#[test]
fn loading_success_sprite_1x() {
    loading_success(default_sprite_path(), 418, 1.0);
}

// gtest: SpriteLoaderParametrized.LoadingSuccess
#[test]
fn loading_success_versatiles_1x() {
    let path = cpp_path("test/fixtures/resources/versatiles-sprite/sprite").to_string_lossy().into_owned();
    loading_success(path, 112, 1.0);
}

// gtest: SpriteLoaderParametrized.LoadingSuccess
#[test]
fn loading_success_versatiles_2x() {
    let path = cpp_path("test/fixtures/resources/versatiles-sprite/sprite").to_string_lossy().into_owned();
    loading_success(path, 112, 2.0);
}

fn expect_error(test: &SpriteLoaderTest) -> Rc<RefCell<Option<String>>> {
    let error_message: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let (e, stop) = (error_message.clone(), test.run_loop.clone());
    *test.observer.sprite_error.borrow_mut() = Some(Box::new(move |error| {
        *e.borrow_mut() = Some(error.to_string());
        stop.stop();
    }));
    error_message
}

// gtest: SpriteLoader.JSONLoadingFail
#[test]
fn json_loading_fail() {
    let test = SpriteLoaderTest::new(1.0);

    test.file_source.set_response(Kind::SpriteImage, successful_response(".png", default_sprite_path(), 1.0));
    test.file_source.set_response(Kind::SpriteJSON, failed_sprite_response);

    let error = expect_error(&test);
    test.run();
    assert_eq!(Some("Failed by the test case".to_owned()), *error.borrow());
}

// gtest: SpriteLoader.ImageLoadingFail
#[test]
fn image_loading_fail() {
    let test = SpriteLoaderTest::new(1.0);

    test.file_source.set_response(Kind::SpriteImage, failed_sprite_response);
    test.file_source.set_response(Kind::SpriteJSON, successful_response(".json", default_sprite_path(), 1.0));

    let error = expect_error(&test);
    test.run();
    assert_eq!(Some("Failed by the test case".to_owned()), *error.borrow());
}

// gtest: SpriteLoader.JSONLoadingCorrupted
#[test]
fn json_loading_corrupted() {
    let test = SpriteLoaderTest::new(1.0);

    test.file_source.set_response(Kind::SpriteImage, successful_response(".png", default_sprite_path(), 1.0));
    test.file_source.set_response(Kind::SpriteJSON, corrupt_sprite_response);

    let error = expect_error(&test);
    test.run();
    // C++: "Failed to parse JSON: Invalid value. at offset 0" (rapidjson's
    // message).
    let error = error.borrow().clone().expect("error");
    assert!(error.starts_with("Failed to parse JSON: "), "{error}");
}

// gtest: SpriteLoader.ImageLoadingCorrupted
#[test]
fn image_loading_corrupted() {
    let test = SpriteLoaderTest::new(1.0);

    test.file_source.set_response(Kind::SpriteImage, corrupt_sprite_response);
    test.file_source.set_response(Kind::SpriteJSON, successful_response(".json", default_sprite_path(), 1.0));

    let error = expect_error(&test);
    test.run();
    // Not asserting on platform-specific error text.
    assert!(error.borrow().is_some());
}

// gtest: SpriteLoader.LoadingCancel
#[test]
fn loading_cancel() {
    let test = SpriteLoaderTest::new(1.0);

    let stop = test.run_loop.clone();
    let end = move |_: &Resource| -> Option<Response> {
        stop.stop();
        None
    };
    let end2 = end.clone();
    test.file_source.set_response(Kind::SpriteImage, end);
    test.file_source.set_response(Kind::SpriteJSON, end2);

    let loaded = Rc::new(RefCell::new(false));
    let l = loaded.clone();
    *test.observer.sprite_loaded.borrow_mut() = Some(Box::new(move |_| {
        *l.borrow_mut() = true;
    }));

    test.run();
    assert!(!*loaded.borrow(), "Should never be called");
    test.end();
}
