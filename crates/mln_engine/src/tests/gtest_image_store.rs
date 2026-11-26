// The C++ ImageManager notifies requestors; the port's ImageStore answers
// waits that ask it again (`resolve`), so a C++ requestor is an
// `ImageWait` here, asked again after each change.

use std::sync::Arc;

use mln_style::bitmap::PremultipliedImage;
use mln_style::hash::HashSet;
use mln_style::image::Impl as ImageImpl;
use mln_style::size::Size;
use mln_util::constants::DEFAULT_ON_DEMAND_IMAGES_CACHE_SIZE;

use crate::image::image_store::{ImageDependencies, ImageStore, ImageType, ImageWait};
use crate::render::events::RendererEvent;
use crate::tests::fixture_log::FixtureLog;

/// Reads a test fixture from the copy of the C++ files (testdata/).
fn read_file(relative: &str) -> Vec<u8> {
    let path = std::path::Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("testdata").join(relative);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
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

/// The images a store asked for and the images it found unused, from its
/// events.
#[derive(Default)]
struct Reported {
    missing: Vec<String>,
    unused: Vec<String>,
}

impl Reported {
    /// Adds what `store` reported since the last call.
    fn take(&mut self, store: &mut ImageStore) -> &Self {
        for event in store.take_events() {
            match event {
                RendererEvent::StyleImageMissing(id) => self.missing.push(id),
                RendererEvent::UnusedStyleImages(ids) => self.unused.extend(ids),
                _ => {}
            }
        }
        self
    }
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
    assert!(store.image("doesnotexist").is_none());
}

// gtest: ImageManager.Basic
#[test]
fn basic() {
    let _log = FixtureLog::new();
    let mut store = ImageStore::default();

    let images = mln_sprite::sprite_parser::parse_sprite(
        "default",
        &mln_image::decode_image(&read_file("test/fixtures/annotations/emerald.png")).unwrap(),
        &read_file("test/fixtures/annotations/emerald.json"),
    )
    .expect("parseSprite");
    for image in &images {
        store.add_image(Arc::clone(image));
        let stored = store.image(&image.id).expect("stored");
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

    assert!(store.image("three").is_some());
    assert!(store.image("two").is_none());
    assert!(store.image("four").is_none());
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
    assert!(store.image("one").expect("one").sdf);
}

// gtest: ImageManager.RemoveReleasesBinPackRect
#[test]
fn remove_releases_bin_pack_rect() {
    let log = FixtureLog::new();
    let mut store = ImageStore::default();

    store.add_image(make_image("big", Size::new(32, 32), 1.0, false));
    assert!(store.image("big").is_some());

    store.remove_image("big");

    store.add_image(make_image("big", Size::new(32, 32), 1.0, false));
    assert!(store.image("big").is_some());
    assert!(log.is_empty());
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
    let mut store = ImageStore::default();
    let mut reported = Reported::default();
    let mut wait = ImageWait::default();
    let mut wants = dependencies(&["pre"]);

    store.begin_update();
    assert!(store.resolve(&wants, &mut wait).is_none());
    assert_eq!(reported.take(&mut store).missing, Vec::<String>::new());

    store.set_loaded(true);
    assert!(store.resolve(&wants, &mut wait).is_none());
    assert_eq!(reported.take(&mut store).missing, ["pre"]);

    store.begin_update();
    assert_eq!(icons(&mut store, &wants, &mut wait), Some(Vec::new()));
    assert_eq!(reported.take(&mut store).missing, ["pre"]);

    // A wait for updated dependencies asks the app again.
    wants.insert("post".to_owned(), ImageType::Icon);
    let mut wait = ImageWait::default();
    assert!(store.resolve(&wants, &mut wait).is_none());
    assert_eq!(reported.take(&mut store).missing, ["pre", "post", "pre"]);

    // Another wait in the same update doesn't ask for them again, and waits
    // as long.
    let mut another = ImageWait::default();
    assert!(store.resolve(&wants, &mut another).is_none());
    assert_eq!(reported.take(&mut store).missing.len(), 3);
    store.begin_update();
    assert!(store.resolve(&wants, &mut another).is_some());
}

// gtest: ImageManager.OnStyleImageMissingAfterSpriteLoaded
#[test]
fn on_style_image_missing_after_sprite_loaded() {
    let mut store = ImageStore::default();
    let mut reported = Reported::default();
    store.set_loaded(true);
    store.begin_update();

    let mut wait = ImageWait::default();
    let wants = dependencies(&["after"]);
    assert!(store.resolve(&wants, &mut wait).is_none());
    assert_eq!(reported.take(&mut store).missing, ["after"]);

    store.begin_update();
    assert_eq!(icons(&mut store, &wants, &mut wait), Some(Vec::new()));
    assert_eq!(reported.take(&mut store).missing.len(), 1);
}

/// The app adds the image it is asked for before the next update: the wait
/// resolves with it.
#[test]
fn an_image_added_for_a_missing_image_event_resolves_the_wait() {
    let mut store = ImageStore::default();
    store.set_loaded(true);
    store.begin_update();

    let mut wait = ImageWait::default();
    let wants = dependencies(&["late"]);
    assert!(store.resolve(&wants, &mut wait).is_none());
    assert_eq!(Reported::default().take(&mut store).missing, ["late"]);

    store.add_image(make_image("late", Size::new(16, 16), 1.0, false));
    store.begin_update();
    assert_eq!(icons(&mut store, &wants, &mut wait), Some(vec!["late".to_owned()]));
}

// gtest: ImageManager.RemoveUnusedStyleImages
#[test]
fn remove_unused_style_images() {
    let mut store = ImageStore::default();
    store.set_loaded(true);
    store.add_image(make_image("sprite", Size::new(16, 16), 1.0, false));

    // Asks for the images, which the app adds.
    let request = |store: &mut ImageStore, ids: &[&str], size: u32| {
        store.begin_update();
        assert!(store.resolve(&dependencies(ids), &mut ImageWait::default()).is_none());
        for id in ids {
            if store.image(id).is_none() {
                store.add_image(make_image(id, Size::new(size, size), 1.0, false));
            }
        }
    };
    let unused = |store: &mut ImageStore, used: &[&str]| {
        store.take_events();
        let used: HashSet<String> = used.iter().map(|id| (*id).to_owned()).collect();
        store.reduce_memory_use(&used);
        let mut unused = Reported::default().take(store).unused.clone();
        unused.sort();
        unused
    };

    request(&mut store, &["missing"], 16);
    assert!(!store.exceeds_cache_size_limit());
    assert_eq!(unused(&mut store, &["missing"]), Vec::<String>::new());
    assert_eq!(unused(&mut store, &[]), ["missing"], "the sprite's images aren't requested ones");
    store.remove_image("missing");

    request(&mut store, &["1024px"], 1024);
    assert!(store.exceeds_cache_size_limit());
    assert_eq!(unused(&mut store, &[]), ["1024px"]);
    store.remove_image("1024px");
    assert!(!store.exceeds_cache_size_limit());

    request(&mut store, &["missing", "1024px"], 16);
    assert_eq!(unused(&mut store, &["missing", "1024px"]), Vec::<String>::new());
    assert_eq!(unused(&mut store, &["missing"]), ["1024px"]);
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

    let mut store = ImageStore::default();
    let mut reported = Reported::default();
    store.set_loaded(true);
    store.add_image(make_image("sprite", Size::new(16, 16), 1.0, false));
    let mut wants = dependencies(&["sprite"]);
    for i in 0..ON_DEMAND_IMAGE_COUNT {
        wants.insert(format!("a{i}"), ImageType::Icon);
    }

    // One wait: asks for the missing images, which the app adds, and
    // resolves with all of them in the next update.
    let request = |store: &mut ImageStore, reported: &mut Reported, wants: &ImageDependencies| {
        store.begin_update();
        let mut wait = ImageWait::default();
        if store.resolve(wants, &mut wait).is_none() {
            for id in &reported.take(store).missing {
                if store.image(id).is_none() {
                    store.add_image(make_image(id, Size::new(ON_DEMAND_IMAGE_SIDE, ON_DEMAND_IMAGE_SIDE), 3.0, false));
                }
            }
            store.begin_update();
        }
        let icons = icons(store, wants, &mut wait).expect("resolved");
        assert_eq!(icons.len(), wants.len());
    };
    let used = |wants: &ImageDependencies| wants.keys().cloned().collect::<HashSet<String>>();

    request(&mut store, &mut reported, &wants);
    assert_eq!(reported.take(&mut store).missing.len(), 6);
    store.reduce_memory_use(&used(&wants));
    assert_eq!(reported.take(&mut store).unused, Vec::<String>::new());

    request(&mut store, &mut reported, &wants);
    assert_eq!(reported.take(&mut store).missing.len(), 6);

    for i in 0..ON_DEMAND_IMAGE_COUNT {
        wants.insert(format!("b{i}"), ImageType::Icon);
    }
    for _round in 0..2 {
        request(&mut store, &mut reported, &wants);
        assert!(store.exceeds_cache_size_limit());
        store.reduce_memory_use(&used(&wants));
        assert_eq!(reported.take(&mut store).unused, Vec::<String>::new());
        assert_eq!(reported.missing.len(), 12);
    }

    // No tile uses them any more.
    store.reduce_memory_use(&HashSet::default());
    assert_eq!(reported.take(&mut store).unused.len(), 12);
    assert!(!reported.unused.contains(&"sprite".to_owned()));
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
