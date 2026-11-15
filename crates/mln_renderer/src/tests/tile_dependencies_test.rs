// Port-only: a geometry tile answers its worker's dependency requests from
// the renderer's stores, and holds the atlas bins of each answer until no
// layout can use them. The worker here is the test, which sends the tile
// what a worker would and reads what the tile sends back.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_actor::{RunLoop, TaggedScheduler, scheduler};
use mln_map_types::{MapDebugOptions, MapMode, TileLodMode, TransformState};
use mln_style_types::image::{ImageType, Impl as ImageImpl};
use mln_tile_id::{CanonicalTileID, OverscaledTileID};
use mln_util::hash::HashMap;
use mln_util::identity::SimpleIdentity;
use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use crate::geometry::feature_index::FeatureIndex;
use crate::gfx::dynamic_texture::DynamicTextureBinId;
use crate::image_store::ImageStoreObserver;
use crate::layout::tile_layout::{Dependencies, Resolved};
use crate::tests::TileDependencyStores;
use crate::tile::geometry_tile::{GeometryTile, LayoutResult};
use crate::tile::geometry_tile_worker::{TileMessage, WorkerMessage};
use crate::tile::raster_dem_tile::{RasterDEMTileMessage, RasterDEMWorkerMessage};
use crate::tile::raster_tile::{RasterTileMessage, RasterWorkerMessage};
use crate::tile::tile::Tile;
use crate::tile::tile_worker_transport::{TileEndpoint, TileWorkerTransport, WorkerEndpoint, WorkerParameters};
use crate::tile_parameters::TileParameters;

/// What the tile sent its worker.
#[derive(Clone, Default)]
struct Sent(Rc<RefCell<Vec<WorkerMessage>>>);

impl Sent {
    fn take(&self) -> Vec<WorkerMessage> {
        std::mem::take(&mut *self.0.borrow_mut())
    }
}

impl WorkerEndpoint for Sent {
    fn send(&self, message: WorkerMessage) {
        self.0.borrow_mut().push(message);
    }
}

/// A transport whose geometry workers are the test's `Sent`.
struct Recording(Sent);

impl TileWorkerTransport for Recording {
    fn create(&self, _parameters: WorkerParameters, _tile: Box<dyn TileEndpoint>) -> Box<dyn WorkerEndpoint> {
        Box::new(self.0.clone())
    }

    fn create_raster(
        &self,
        _tile: Box<dyn TileEndpoint<RasterTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterWorkerMessage>> {
        panic!("geometry tiles only")
    }

    fn create_raster_dem(
        &self,
        _tile: Box<dyn TileEndpoint<RasterDEMTileMessage>>,
    ) -> Box<dyn WorkerEndpoint<RasterDEMWorkerMessage>> {
        panic!("geometry tiles only")
    }
}

/// Counts the images the store asks the app for.
#[derive(Default)]
struct Asked(RefCell<Vec<String>>);

impl ImageStoreObserver for Asked {
    fn on_style_image_missing(&self, id: &str) {
        self.0.borrow_mut().push(id.to_owned());
    }
}

struct Test {
    run_loop: RunLoop,
    thread_pool: TaggedScheduler,
    sent: Sent,
    stores: TileDependencyStores,
    asked: Rc<Asked>,
    tile: GeometryTile,
}

fn image(id: &str) -> Arc<ImageImpl> {
    Arc::new(
        ImageImpl::new(
            id.to_owned(),
            PremultipliedImage::new(Size::new(4, 4)),
            1.0,
            false,
            vec![],
            vec![],
            None,
            None,
            None,
        )
        .expect("image"),
    )
}

fn icons(ids: &[&str]) -> Dependencies {
    Dependencies {
        glyphs: mln_text::glyph::GlyphDependencies::default(),
        images: ids.iter().map(|id| ((*id).to_owned(), ImageType::Icon)).collect(),
        unavailable_images: VecSet::new(),
    }
}

impl Test {
    /// A tile whose worker was told that `available` exist, and a loaded
    /// sprite of `images`.
    fn new(available: &[&str], images: &[&str]) -> Self {
        let run_loop = RunLoop::new();
        let thread_pool = TaggedScheduler::new(scheduler::get_background(), SimpleIdentity::new());
        let sent = Sent::default();
        let mut stores = TileDependencyStores::new(&run_loop);
        let asked = Rc::new(Asked::default());
        stores.images.set_observer(Some(Rc::<Asked>::clone(&asked)));
        for id in images {
            stores.images.add_image(image(id));
        }
        stores.images.set_loaded(true);
        let transform_state = TransformState::default();
        let parameters = TileParameters {
            pixel_ratio: 1.0,
            debug_options: MapDebugOptions::NO_DEBUG,
            transform_state: &transform_state,
            file_source: None,
            scheduler: run_loop.scheduler(),
            mode: MapMode::Static,
            prefetch_zoom_delta: 0,
            tile_lod_min_radius: 3.0,
            tile_lod_scale: 1.0,
            tile_lod_pitch_threshold: (60.0 / 180.0) * std::f64::consts::PI,
            tile_lod_zoom_shift: 0.0,
            tile_lod_mode: TileLodMode::Default,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            global_state: None,
            is_update_synchronous: true,
            capture_rendered_features: false,
            available_images: Arc::new(available.iter().map(|id| (*id).to_owned()).collect::<VecSet<String>>()),
            font_faces: None,
            tile_worker_transport: Rc::new(Recording(sent.clone())),
            thread_pool: thread_pool.clone(),
        };
        let id = OverscaledTileID::new(0, 0, CanonicalTileID::new(0, 0, 0));
        let tile = GeometryTile::new(id, "s".to_owned(), &parameters, None);
        Test { run_loop, thread_pool, sent, stores, asked, tile }
    }

    /// The worker asks for `dependencies` with request id `request`.
    fn request(&self, dependencies: Dependencies, request: u64) {
        self.tile.inner().handle_worker_message(TileMessage::GetDependencies { dependencies, request });
    }

    /// The renderer's update: the stores answer what they can.
    fn update(&mut self) {
        self.stores.update();
        self.stores.resolve(&self.tile);
    }

    /// The answers the tile sent, by request id.
    fn answers(&self) -> HashMap<u64, Resolved> {
        self.sent
            .take()
            .into_iter()
            .filter_map(|message| match message {
                WorkerMessage::Resolved { resolved, request } => Some((request, resolved)),
                _ => None,
            })
            .collect()
    }

    /// The worker's layout, after its last request `request`.
    fn layout(&self, request: u64, resolved: bool) {
        let result = LayoutResult::new(HashMap::default(), Arc::new(FeatureIndex::new(None)));
        self.tile.inner().handle_worker_message(TileMessage::Layout { result, correlation_id: 0, request, resolved });
    }

    /// The id of the bin the atlas packs `id` into now.
    fn bin_of(&mut self, id: &str) -> DynamicTextureBinId {
        let icons = [(id.to_owned(), self.stores.images.get_image(id).cloned().expect("image"))].into_iter().collect();
        let page = self.stores.atlas.pack_images(&icons, &mln_style_types::image::ImageMap::default());
        page.lease.texture_handles()[0].get_id()
    }
}

impl Drop for Test {
    fn drop(&mut self) {
        self.run_loop.run_once();
        self.thread_pool.wait_for_empty();
    }
}

/// An answer holds its bins until a layout uses a later one: a bin the
/// worker was told about can't be given to another image while the worker
/// may still lay out with it.
#[test]
fn an_answer_holds_its_bins_until_a_layout_uses_a_later_one() {
    let mut test = Test::new(&["a", "b"], &["a", "b"]);
    test.request(icons(&["a"]), 1);
    test.update();
    let first = test.answers();
    assert!(first[&1].icons.contains_key("a"));
    let bin = test.bin_of("a");

    // The worker parsed again before laying out with the first answer.
    test.request(icons(&["b"]), 2);
    test.update();
    assert!(test.answers()[&2].icons.contains_key("b"));
    test.stores.atlas.free_released();
    assert_eq!(test.bin_of("a"), bin, "the first answer still holds the bin");

    test.layout(2, true);
    assert!(test.tile.create_render_data().get_atlas_textures().is_some());
    test.stores.atlas.free_released();
    assert_ne!(test.bin_of("a"), bin, "no layout can use the first answer: its bin was freed");
}

/// The stores answer only when they have something new: a tile that waits
/// for a missing image isn't answered again and again.
#[test]
fn a_missing_image_resolves_as_missing_in_the_next_update() {
    let mut test = Test::new(&[], &[]);
    test.request(icons(&["missing"]), 1);
    test.update();
    assert!(test.answers().is_empty());
    assert_eq!(*test.asked.0.borrow(), ["missing"]);

    assert!(!test.stores.resolve(&test.tile), "nothing changed");
    assert!(test.answers().is_empty());

    test.update();
    let answers = test.answers();
    assert!(answers[&1].icons.is_empty());
    assert_eq!(test.asked.0.borrow().len(), 1);
    let latest =
        test.tile.inner().latest_dependencies(|dependencies, answered, _| (dependencies.images.len(), answered));
    assert_eq!(latest, Some((1, true)));
}

/// A request from a layout that branched on an image being unavailable,
/// which came since the worker was told which exist, lays the tile out
/// again, with the images that exist now, instead of being answered.
#[test]
fn a_request_made_with_outdated_images_lays_out_again() {
    let mut test = Test::new(&[], &["late", "literal"]);
    // An icon that no `image` expression branched on is just answered.
    test.request(icons(&["literal"]), 1);
    test.update();
    assert!(test.answers()[&1].icons.contains_key("literal"));

    let mut dependencies = icons(&["late"]);
    dependencies.unavailable_images.insert("late".to_owned());
    test.request(dependencies, 2);
    test.update();
    let sent = test.sent.take();
    let [WorkerMessage::SetAvailableImages { available_images, .. }] = sent.as_slice() else {
        panic!("expected a relayout, got {} messages", sent.len());
    };
    assert!(available_images.contains("late"));
    assert!(test.tile.base().pending.get());
    assert_eq!(test.tile.inner().latest_dependencies(|_, _, _| ()), None);
}

/// A layout that used no answer abandons the worker's requests until then.
#[test]
fn a_layout_without_an_answer_abandons_the_requests() {
    let mut test = Test::new(&["a"], &["a"]);
    test.request(icons(&["a"]), 1);
    test.layout(1, false);
    assert_eq!(test.tile.inner().latest_dependencies(|_, _, _| ()), None);
    test.update();
    assert!(test.answers().is_empty());
}

/// A request naming an image that went since the worker was told which
/// exist lays the tile out again too: the layout may have taken it as there.
#[test]
fn a_request_naming_a_removed_image_lays_out_again() {
    let mut test = Test::new(&["gone"], &[]);
    test.request(icons(&["gone"]), 1);
    test.update();
    let sent = test.sent.take();
    let [WorkerMessage::SetAvailableImages { available_images, .. }] = sent.as_slice() else {
        panic!("expected a relayout, got {} messages", sent.len());
    };
    assert!(available_images.is_empty());
}
