// C++ `GeometryTile` is both the tile the TilePyramid owns and the target of
// its worker's replies (an actor reference to `this`). Here the state the
// replies change is `GeometryTileInner`, an `Rc` registered as the target of
// the tile's mailbox on the render thread; `GeometryTile` owns it, the
// mailbox and the worker actor. Correlation between GeometryTile and
// GeometryTileWorker works as in C++ (see the C++ file's comment):
// `correlationID` for the tile's pending state.
//
// The tile reaches its worker through the `TileWorkerTransport` of its
// TileParameters (tile_worker_transport.rs): it sends `WorkerMessage`s to
// the transport's endpoint, and the worker's `TileMessage`s come back
// through the tile's mailbox (`handle_worker_message`). As in C++, the
// worker runs synchronously on the render thread when the source asks for
// it (`TileParameters::isUpdateSynchronous`, e.g. a GeoJSON source with
// `synchronousUpdate`); its messages to the tile are then direct calls.
//
// symbols: the worker asks for the glyphs and images its layout needs
// (`TileMessage::GetDependencies`). The tile waits until the renderer's
// update resolves them (`resolve_dependencies`, through the glyph and image
// stores), packs them into one glyph page and one image page of the atlas,
// and sends the worker their positions (`WorkerMessage::Resolved`). It holds
// leases on the bins until no layout can use them.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_actor::{ActorRef, LocalObject, Mailbox, TaggedScheduler};
use mln_map_types::{MapDebugOptions, MapMode};
use mln_storage::FileSource;
use mln_style::expression::GlobalStateMap;
use mln_style::layer;
use mln_style::layer::layer_type_info;
use mln_style::layer_properties::LayerProperties;
use mln_style_types::VisibilityType;
use mln_style_types::image::{ImagePosition, ImagePositions};
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::hash::HashMap;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

use crate::bucket::BucketPtr;
use crate::bucket_geometry::BucketGeometry;
use crate::geometry::feature_index::FeatureIndex;
use crate::gfx::context::Context;
use crate::gfx::dynamic_texture_atlas::{AtlasLease, DynamicTextureAtlas};
use crate::image_store::{ImageStore, ImageWait};
use crate::layout::tile_layout::{Dependencies, Resolved};
use crate::render_layer::LayerRenderData;
use crate::render_orchestrator::drop_on_workers;
use crate::text::glyph_store::{GlyphStore, GlyphWait};
use crate::tile::geometry_tile_worker::{TileMessage, WorkerMessage};
use crate::tile::tile::{Kind, Tile, TileBase, TileOperation};
use crate::tile::tile_observer::TileObserver;
use crate::tile::tile_worker_transport::{SynchronousTile, TileEndpoint, WorkerEndpoint, WorkerParameters};
use crate::tile_parameters::TileParameters;
use crate::tile_render_data::{TileAtlasTextures, TileRenderData};

/// A worker's layout: each layer's bucket, and the feature index.
#[derive(Clone)]
pub struct LayoutResult {
    pub(crate) layer_render_data: HashMap<String, LayerRenderData>,
    pub(crate) feature_index: Arc<FeatureIndex>,
}

impl LayoutResult {
    #[must_use]
    pub(crate) fn new(layer_render_data: HashMap<String, LayerRenderData>, feature_index: Arc<FeatureIndex>) -> Self {
        LayoutResult { layer_render_data, feature_index }
    }

    pub(crate) fn get_layer_render_data(&self, layer_impl: &dyn layer::Impl) -> Option<&LayerRenderData> {
        let result = self.layer_render_data.get(&layer_impl.id)?;
        if !std::ptr::eq(result.layer_properties.base_impl.get_type_info(), layer_impl.get_type_info()) {
            // Layer data might be outdated, see issue #12432.
            return None;
        }
        Some(result)
    }
}

/// The atlas pages a layout draws its glyphs and images from. Dropping it
/// gives their bins back.
pub(crate) struct TileAtlases {
    glyphs: AtlasLease,
    images: AtlasLease,
    /// Where the layout's patterns are, which drawing them and feature state
    /// read.
    pattern_positions: ImagePositions,
}

/// The layout a tile shows: its worker's result, the atlas pages it draws
/// from if it waited for glyphs or images, and the render thread's copies of
/// its bucket geometry.
#[derive(Clone)]
pub(crate) struct TileLayout {
    pub(crate) result: LayoutResult,
    pub(crate) atlases: Option<Rc<TileAtlases>>,
    textures: Rc<TileAtlasTextures>,
    bucket_geometry: Rc<BucketGeometry>,
}

impl TileLayout {
    fn new(result: LayoutResult, atlases: Option<TileAtlases>) -> Self {
        let textures = atlases.as_ref().map_or_else(TileAtlasTextures::default, |atlases| {
            TileAtlasTextures::new(Rc::clone(atlases.glyphs.page_texture()), Rc::clone(atlases.images.page_texture()))
        });
        TileLayout {
            result,
            atlases: atlases.map(Rc::new),
            textures: Rc::new(textures),
            bucket_geometry: Rc::default(),
        }
    }

    fn pattern_positions(&self) -> Option<&ImagePositions> {
        self.atlases.as_ref().map(|atlases| &atlases.pattern_positions)
    }
}

/// Port-only: lets go of a tile's layout. The last reference frees the
/// buckets and the feature index on the orchestrator's workers, since a view
/// that drops many tiles would otherwise spend frames freeing them. The
/// atlas leases drop here, on the render thread.
fn release_layout(layout: Rc<TileLayout>, thread_pool: &TaggedScheduler) {
    if let Some(TileLayout { result: LayoutResult { layer_render_data, feature_index }, atlases, .. }) =
        Rc::into_inner(layout)
    {
        drop_on_workers(thread_pool, (layer_render_data, feature_index));
        drop(atlases);
    }
}

struct GeometryTileRenderData {
    layout: Option<Rc<TileLayout>>,
    thread_pool: TaggedScheduler,
}

impl Drop for GeometryTileRenderData {
    fn drop(&mut self) {
        if let Some(layout) = self.layout.take() {
            release_layout(layout, &self.thread_pool);
        }
    }
}

impl TileRenderData for GeometryTileRenderData {
    fn get_atlas_textures(&self) -> Option<&Rc<TileAtlasTextures>> {
        self.layout.as_ref().map(|layout| &layout.textures)
    }

    fn get_pattern(&self, pattern: &str) -> Option<ImagePosition> {
        self.layout.as_ref()?.pattern_positions()?.get(pattern).cloned()
    }

    fn get_layer_render_data(&self, layer_impl: &dyn layer::Impl) -> Option<&LayerRenderData> {
        self.layout.as_ref().and_then(|layout| layout.result.get_layer_render_data(layer_impl))
    }

    fn get_bucket(&self, layer_impl: &dyn layer::Impl) -> Option<&BucketPtr> {
        self.get_layer_render_data(layer_impl).map(|data| &data.bucket)
    }

    fn get_feature_index(&self) -> Option<Arc<FeatureIndex>> {
        self.layout.as_ref().map(|layout| Arc::clone(&layout.result.feature_index))
    }

    fn get_bucket_geometry(&self) -> Option<&BucketGeometry> {
        self.layout.as_ref().map(|layout| &*layout.bucket_geometry)
    }

    fn upload(&self, context: &Context) {
        let Some(layout) = &self.layout else {
            return;
        };

        for entry in layout.result.layer_render_data.values() {
            if entry.bucket.needs_upload() {
                entry.bucket.upload(context);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FadeState {
    Loaded,
    NeedsFirstPlacement,
    NeedsSecondPlacement,
    CanRemove,
}

/// What the tiles resolve their worker's dependency requests with.
pub(crate) struct DependencyStores<'a> {
    pub(crate) glyphs: &'a mut GlyphStore,
    pub(crate) images: &'a mut ImageStore,
    pub(crate) atlas: &'a mut DynamicTextureAtlas,
}

/// The worker's latest dependency request.
struct DependencyRequest {
    id: u64,
    dependencies: Dependencies,
    /// `None` once answered.
    wait: Option<DependencyWait>,
    /// Whether the answer lacked glyphs of a range that failed to load.
    glyphs_failed: bool,
}

/// A dependency request's wait for the stores.
#[derive(Default)]
struct DependencyWait {
    glyphs: GlyphWait,
    images: ImageWait,
    /// The stores' revisions at the last try, which only another revision
    /// can change the outcome of.
    revisions: Option<(u64, u64)>,
}

/// The tile's side of its worker's dependency requests.
struct DependencyState {
    /// The images the worker was last told exist.
    available_images: Arc<VecSet<String>>,
    latest: Option<DependencyRequest>,
    /// The atlas pages of each answer a layout may still use, by request id.
    sent: Vec<(u64, TileAtlases)>,
}

/// The render-thread state of a `GeometryTile`, also the target of its
/// worker's replies.
pub(crate) struct GeometryTileInner {
    pub(crate) base: TileBase,

    file_source: Option<Arc<dyn FileSource>>,
    dependencies: RefCell<DependencyState>,

    /// Used to signal the worker that it should abandon parsing this tile as
    /// soon as possible.
    obsolete: Arc<AtomicBool>,

    /// The tile's worker, behind its transport.
    worker: Box<dyn WorkerEndpoint>,
    mailbox: Arc<Mailbox>,

    correlation_id: Cell<u64>,
    /// The worker has data to parse: set by `set_data`, cleared by `reset`.
    has_data: Cell<bool>,
    /// The tileset has no tile here (`LoaderTile::set_missing`): the tile
    /// lays out without data but isn't renderable.
    pub(crate) missing: Cell<bool>,

    layout: RefCell<Option<Rc<TileLayout>>>,
    /// Where the layout's data is freed (`release_layout`).
    thread_pool: TaggedScheduler,

    mode: MapMode,

    show_collision_boxes: Cell<bool>,
    subdivision_granularity: Cell<SubdivisionGranularitySetting>,
    pixel_ratio: Cell<f32>,

    fade_state: Cell<FadeState>,
}

impl GeometryTileInner {
    /// The `obsolete` flag.
    pub(crate) fn is_obsolete(&self) -> bool {
        self.obsolete.load(Ordering::SeqCst)
    }

    fn mark_obsolete(&self) {
        self.obsolete.store(true, Ordering::SeqCst);
        // `mailbox->abandon()`: no reply reaches the tile any more.
        self.mailbox.close();
    }

    pub(crate) fn set_error(&self, err: Error) {
        self.base.loaded.set(true);
        // Without data, the worker has nothing to parse, so the tile is
        // complete.
        if !self.has_data.get() {
            self.base.pending.set(false);
        }
        self.base.observer().on_tile_error(&self.base, err);
    }

    pub(crate) fn set_data(&self, data: Option<TileData>) {
        if self.obsolete.load(Ordering::SeqCst) {
            return;
        }

        if !self.base.pending.get() {
            self.base.on_tile_action(TileOperation::StartParse);
        }

        // Mark the tile as pending again if it was complete before to
        // prevent signaling a complete state despite pending parse
        // operations.
        self.base.pending.set(true);
        self.has_data.set(true);

        self.correlation_id.set(self.correlation_id.get() + 1);
        let correlation_id = self.correlation_id.get();
        let available_images = Arc::clone(&self.dependencies.borrow().available_images);
        self.worker.send(WorkerMessage::SetData { data, available_images, correlation_id });
    }

    /// Resets the tile's data and layers and leaves the tile in pending
    /// state, waiting for the new data and layers to come.
    pub(crate) fn reset(&self) {
        // If there is pending work, indicate that work has been cancelled.
        // Clear the pending status.
        if self.base.pending.get() {
            self.base.on_tile_action(TileOperation::Cancelled);
            self.base.pending.set(false);
        }

        // Reset the tile to an unloaded state to avoid signaling completion
        // after clearing the tile's pending status.
        self.base.loaded.set(false);
        self.has_data.set(false);

        // Reset the worker to the `NeedsParse` state.
        self.correlation_id.set(self.correlation_id.get() + 1);
        let correlation_id = self.correlation_id.get();
        self.worker.send(WorkerMessage::Reset { correlation_id });
    }

    /// Installs a layout. `request` is the worker's last dependency request
    /// and `resolved` whether the layout used its answer.
    fn on_layout(&self, result: LayoutResult, result_correlation_id: u64, request: u64, resolved: bool) {
        let atlases = {
            let mut dependencies = self.dependencies.borrow_mut();
            let state = &mut *dependencies;
            let atlases = resolved
                .then(|| state.sent.iter().position(|(id, _)| *id == request))
                .flatten()
                .map(|index| state.sent.remove(index).1);
            // The worker answers only its last request, which this layout
            // used or abandoned.
            state.sent.retain(|(id, _)| *id > request);
            if !resolved && state.latest.as_ref().is_some_and(|latest| latest.id <= request) {
                state.latest = None;
            }
            atlases
        };

        self.base.loaded.set(true);
        self.base.renderable.set(!self.missing.get());
        if result_correlation_id == self.correlation_id.get() {
            self.base.pending.set(false);
            self.base.on_tile_action(TileOperation::EndParse);
        }

        *self.layout.borrow_mut() = Some(Rc::new(TileLayout::new(result, atlases)));
        self.base.feature_state_version.set(0);

        self.base.observer().on_tile_changed(&self.base);
    }

    pub(crate) fn on_error(&self, err: Error, result_correlation_id: u64) {
        self.base.loaded.set(true);
        if result_correlation_id == self.correlation_id.get() {
            self.base.pending.set(false);
            self.base.on_tile_action(TileOperation::Error);
        }
        self.base.observer().on_tile_error(&self.base, err);
    }

    /// Waits for the renderer's next update to resolve what the worker asked
    /// for.
    fn on_dependencies_requested(&self, dependencies: Dependencies, request: u64) {
        self.dependencies.borrow_mut().latest = Some(DependencyRequest {
            id: request,
            dependencies,
            wait: Some(DependencyWait::default()),
            glyphs_failed: false,
        });
        self.base.observer().on_dependencies_requested(&self.base);
    }

    /// Answers the worker's dependency request once the stores have
    /// everything it needs, or know it missing, packing it into the atlas.
    /// Does nothing for a tile that waits for nothing, or whose stores
    /// haven't changed since it last tried. Returns whether it answered.
    pub(crate) fn resolve_dependencies(&self, stores: &mut DependencyStores<'_>) -> bool {
        if self.is_obsolete() {
            return false;
        }
        let revisions = (stores.glyphs.revision(), stores.images.revision());
        let (resolved, request) = {
            let mut dependencies = self.dependencies.borrow_mut();
            let state = &mut *dependencies;
            let Some(latest) = state.latest.as_mut() else {
                return false;
            };
            let Some(wait) = latest.wait.as_mut() else {
                return false;
            };
            if wait.revisions == Some(revisions) {
                return false;
            }

            // A layout that branched on an image that came or went since the
            // worker was told which exist isn't the one to answer: lay it out
            // again. An image it names but didn't branch on just resolves.
            let current = stores.images.available_images();
            let sent = &state.available_images;
            if latest.dependencies.unavailable_images.iter().any(|id| current.contains(id))
                || latest.dependencies.images.keys().any(|id| sent.contains(id) && !current.contains(id))
            {
                let current = Arc::clone(current);
                state.latest = None;
                drop(dependencies);
                self.relayout(current);
                return false;
            }

            let glyphs =
                stores.glyphs.resolve(&latest.dependencies.glyphs, &mut wait.glyphs, self.file_source.as_ref());
            let images = stores.images.resolve(&latest.dependencies.images, &mut wait.images);
            let (Some(glyphs), Some(images)) = (glyphs, images) else {
                wait.revisions = Some((stores.glyphs.revision(), stores.images.revision()));
                return false;
            };

            let glyph_page = stores.atlas.pack_glyphs(&glyphs.glyphs);
            let image_page = stores.atlas.pack_images(&images.icons, &images.patterns);
            latest.wait = None;
            latest.glyphs_failed = glyphs.failed;
            let request = latest.id;
            state.sent.push((
                request,
                TileAtlases {
                    glyphs: glyph_page.lease,
                    images: image_page.lease,
                    pattern_positions: image_page.patterns.clone(),
                },
            ));
            let resolved = Resolved {
                glyphs: glyph_page.positions,
                shaping: glyphs.shaping,
                icons: image_page.icons,
                patterns: image_page.patterns,
            };
            (resolved, request)
        };
        self.worker.send(WorkerMessage::Resolved { resolved, request });
        true
    }

    /// Calls `f` with the worker's latest dependencies, which name the
    /// images the tile's layout changes with, whether they were answered,
    /// and whether their answer lacked glyphs of a range that failed to
    /// load. `None` if the worker asked for none.
    pub(crate) fn latest_dependencies<R>(&self, f: impl FnOnce(&Dependencies, bool, bool) -> R) -> Option<R> {
        let dependencies = self.dependencies.borrow();
        let latest = dependencies.latest.as_ref()?;
        Some(f(&latest.dependencies, latest.wait.is_none(), latest.glyphs_failed))
    }

    /// Parses again with `available_images`: the tile is pending until its
    /// layout arrives.
    pub(crate) fn relayout(&self, available_images: Arc<VecSet<String>>) {
        if self.is_obsolete() {
            return;
        }

        if !self.base.pending.get() {
            self.base.pending.set(true);
            self.base.on_tile_action(TileOperation::StartParse);
        }

        self.correlation_id.set(self.correlation_id.get() + 1);
        let correlation_id = self.correlation_id.get();
        self.dependencies.borrow_mut().available_images = Arc::clone(&available_images);
        self.worker.send(WorkerMessage::SetAvailableImages { available_images, correlation_id });
    }

    /// Runs the member function a worker's message stands for.
    pub(crate) fn handle_worker_message(&self, message: TileMessage) {
        match message {
            TileMessage::Layout { result, correlation_id, request, resolved } => {
                self.on_layout(result, correlation_id, request, resolved);
            }
            TileMessage::Error { error, correlation_id } => self.on_error(error, correlation_id),
            TileMessage::GetDependencies { dependencies, request } => {
                self.on_dependencies_requested(dependencies, request);
            }
        }
    }

    pub(crate) fn get_feature_index(&self) -> Option<Arc<FeatureIndex>> {
        self.layout.borrow().as_ref().map(|layout| Arc::clone(&layout.result.feature_index))
    }
}

pub(crate) struct GeometryTile {
    inner: Rc<GeometryTileInner>,
}

impl GeometryTile {
    #[must_use]
    pub(crate) fn new(
        id: OverscaledTileID,
        source_id: String,
        parameters: &TileParameters<'_>,
        observer: Option<Rc<dyn TileObserver>>,
    ) -> Self {
        let obsolete = Arc::new(AtomicBool::new(false));
        let mailbox = Mailbox::new();
        let show_collision_boxes = parameters.debug_options.contains(MapDebugOptions::COLLISION);
        let synchronous = parameters.is_update_synchronous;
        let worker_parameters = WorkerParameters {
            id,
            obsolete: Arc::clone(&obsolete),
            mode: parameters.mode,
            pixel_ratio: parameters.pixel_ratio,
            show_collision_boxes,
            font_faces: parameters.font_faces.clone(),
            capture_rendered_features: parameters.capture_rendered_features,
            synchronous,
        };
        let inner = Rc::new_cyclic(|self_weak: &Weak<GeometryTileInner>| {
            // OptionalActorRef<GeometryTile>(isUpdateSynchronous, *this,
            // mailbox).
            let tile: Box<dyn TileEndpoint> = if synchronous {
                Box::new(SynchronousTile(LocalObject::new(Weak::clone(self_weak))))
            } else {
                Box::new(ActorRef::<GeometryTileInner>::new(&mailbox))
            };
            GeometryTileInner {
                base: TileBase::new(Kind::Geometry, id, source_id, observer),
                file_source: parameters.file_source.clone(),
                dependencies: RefCell::new(DependencyState {
                    available_images: Arc::clone(&parameters.available_images),
                    latest: None,
                    sent: Vec::new(),
                }),
                obsolete,
                worker: parameters.tile_worker_transport.create(worker_parameters, tile),
                mailbox: Arc::clone(&mailbox),
                correlation_id: Cell::new(0),
                has_data: Cell::new(false),
                missing: Cell::new(false),
                layout: RefCell::new(None),
                thread_pool: parameters.thread_pool.clone(),
                mode: parameters.mode,
                show_collision_boxes: Cell::new(show_collision_boxes),
                subdivision_granularity: Cell::new(SubdivisionGranularitySetting::none()),
                pixel_ratio: Cell::new(parameters.pixel_ratio),
                fade_state: Cell::new(FadeState::Loaded),
            }
        });
        // `mailbox(std::make_shared<Mailbox>(*Scheduler::GetCurrent()))`
        mailbox.open(&parameters.scheduler, Rc::<GeometryTileInner>::clone(&inner));
        GeometryTile { inner }
    }

    #[must_use]
    pub(crate) fn inner(&self) -> &Rc<GeometryTileInner> {
        &self.inner
    }

    #[cfg(test)]
    pub(crate) fn set_error(&self, err: Error) {
        self.inner.set_error(err);
    }

    #[cfg(test)]
    pub(crate) fn set_data(&self, data: Option<TileData>) {
        self.inner.set_data(data);
    }

    pub(crate) fn reset(&self) {
        self.inner.reset();
    }

    #[must_use]
    pub(crate) fn get_feature_index(&self) -> Option<Arc<FeatureIndex>> {
        self.inner.get_feature_index()
    }
}

impl Drop for GeometryTile {
    fn drop(&mut self) {
        self.inner.mark_obsolete();

        if self.inner.base.pending.get() {
            // This tile never finished loading or was abandoned, emit a
            // cancellation event
            self.inner.base.on_tile_action(TileOperation::Cancelled);
        }

        let layout = self.inner.layout.borrow_mut().take();
        if let Some(layout) = layout {
            release_layout(layout, &self.inner.thread_pool);
        }
    }
}

impl Tile for GeometryTile {
    fn base(&self) -> &TileBase {
        &self.inner.base
    }

    fn create_render_data(&self) -> Box<dyn TileRenderData> {
        Box::new(GeometryTileRenderData {
            layout: self.inner.layout.borrow().clone(),
            thread_pool: self.inner.thread_pool.clone(),
        })
    }

    fn cancel(&self) {
        self.inner.mark_obsolete();
    }

    fn set_layers(
        &self,
        layers: &[Arc<dyn LayerProperties>],
        global_state: Option<Arc<GlobalStateMap>>,
        available_images: &Arc<VecSet<String>>,
    ) {
        let inner = &self.inner;
        // Mark the tile as pending again if it was complete before to
        // prevent signaling a complete state despite pending parse
        // operations. A tile whose data failed to load has nothing to parse.
        if !inner.base.pending.get() && (inner.has_data.get() || !inner.base.loaded.get()) {
            inner.base.pending.set(true);
            inner.base.on_tile_action(TileOperation::StartParse);
        }

        let id = inner.base.id();
        let mut impls: Vec<Arc<dyn LayerProperties>> = Vec::with_capacity(layers.len());

        for layer in layers {
            // Skip irrelevant layers.
            let layer_impl = &layer.base_impl;
            debug_assert_ne!(layer_impl.get_type_info().source, layer_type_info::Source::NotRequired);
            debug_assert_eq!(layer_impl.source, inner.base.source_id);
            debug_assert_ne!(layer_impl.visibility, VisibilityType::None);
            let overscaled_z = f32::from(id.overscaled_z);
            if overscaled_z < layer_impl.min_zoom.floor() || overscaled_z >= layer_impl.max_zoom.ceil() {
                continue;
            }

            impls.push(Arc::clone(layer));
        }

        inner.correlation_id.set(inner.correlation_id.get() + 1);
        let correlation_id = inner.correlation_id.get();
        let available_images = Arc::clone(available_images);
        inner.dependencies.borrow_mut().available_images = Arc::clone(&available_images);
        let subdivision_granularity = inner.subdivision_granularity.get();
        inner.worker.send(WorkerMessage::SetLayers {
            layers: impls,
            global_state,
            available_images,
            subdivision_granularity,
            pixel_ratio: inner.pixel_ratio.get(),
            correlation_id,
        });
    }

    fn set_show_collision_boxes(&self, show_collision_boxes: bool) {
        let inner = &self.inner;
        if inner.show_collision_boxes.get() != show_collision_boxes {
            inner.show_collision_boxes.set(show_collision_boxes);
            inner.correlation_id.set(inner.correlation_id.get() + 1);
            let correlation_id = inner.correlation_id.get();
            inner.worker.send(WorkerMessage::SetShowCollisionBoxes { show_collision_boxes, correlation_id });
        }
    }

    fn set_subdivision_granularity(&self, value: &SubdivisionGranularitySetting) {
        self.inner.subdivision_granularity.set(*value);
    }

    fn set_pixel_ratio(&self, pixel_ratio: f32) -> bool {
        self.inner.pixel_ratio.replace(pixel_ratio) != pixel_ratio
    }

    #[cfg(test)]
    fn set_observer(&self, observer: Rc<dyn TileObserver>) {
        self.inner.base.set_observer(observer);
    }

    // layers: feature state
    fn set_feature_state(&self, states: &crate::bucket::LayerFeatureStates) {
        let layout = self.inner.layout.borrow();
        let Some(layout) = layout.as_ref() else {
            return;
        };
        // getData(): latestFeatureIndex->getData()
        let Some(layers) = layout.result.feature_index.get_data() else {
            return;
        };
        if states.is_empty() {
            return;
        }
        let no_patterns = ImagePositions::default();
        let pattern_positions = layout.pattern_positions().unwrap_or(&no_patterns);

        for (layer_id, render_data) in &layout.result.layer_render_data {
            let source_layer_id = &render_data.layer_properties.base_impl.source_layer;
            // The worker got the layer from the same data.
            if let Ok(Some(source_layer)) = layers.layer(source_layer_id) {
                let Some(feature_states) = states.get(source_layer_id) else {
                    continue;
                };
                if feature_states.is_empty() {
                    continue;
                }
                if render_data.bucket.has_data() {
                    render_data.bucket.update(feature_states, &source_layer, layer_id, pattern_positions);
                }
            }
        }
    }

    fn layer_properties_updated(&self, layer_properties: &Arc<dyn LayerProperties>) -> bool {
        let mut layout = self.inner.layout.borrow_mut();
        let Some(layout) = layout.as_mut() else {
            return false;
        };
        let layer_impl = &layer_properties.base_impl;
        let Some(render_data) = layout.result.get_layer_render_data(&**layer_impl) else {
            return false;
        };

        if !Arc::ptr_eq(&render_data.layer_properties, layer_properties)
            && render_data.layer_properties.constants_mask() == layer_properties.constants_mask()
        {
            // C++ updates the shared LayoutResult in place; render data
            // created from it before (the previous frame's RenderTiles) see
            // the change too, but are replaced in the next prepare. The port
            // updates a copy if the result is shared.
            let layout = Rc::make_mut(layout);
            if let Some(render_data) = layout.result.layer_render_data.get_mut(&layer_impl.id) {
                render_data.layer_properties = Arc::clone(layer_properties);
            }
        }

        true
    }

    fn hold_for_fade(&self) -> bool {
        self.inner.mode == MapMode::Continuous
            && matches!(self.inner.fade_state.get(), FadeState::NeedsFirstPlacement | FadeState::NeedsSecondPlacement)
    }

    fn mark_rendered_ideal(&self) {
        self.inner.fade_state.set(FadeState::Loaded);
    }

    fn mark_rendered_previously(&self) {
        if self.inner.fade_state.get() == FadeState::Loaded {
            self.inner.fade_state.set(FadeState::NeedsFirstPlacement);
        }
    }

    fn performed_fade_placement(&self) {
        match self.inner.fade_state.get() {
            FadeState::NeedsFirstPlacement => self.inner.fade_state.set(FadeState::NeedsSecondPlacement),
            FadeState::NeedsSecondPlacement => self.inner.fade_state.set(FadeState::CanRemove),
            _ => {}
        }
    }

    // query
    fn get_query_padding(&self, layers: &QueryLayers<'_>) -> f32 {
        let layout = self.inner.layout.borrow();
        let mut query_padding: f32 = 0.0;
        for layer in layers.values() {
            let Some(data) =
                layout.as_ref().and_then(|layout| layout.result.get_layer_render_data(&*layer.base().base_impl))
            else {
                continue;
            };
            if data.bucket.has_data() {
                // std::max(queryPadding, radius)
                let radius = data.bucket.get_query_radius(*layer);
                if query_padding < radius {
                    query_padding = radius;
                }
            }
        }
        query_padding
    }

    fn query_rendered_features(
        &self,
        result: &mut QueryResults,
        query_geometry: &mln_tile_id::geometry_tile_data::GeometryCoordinates,
        transform_state: &mln_map_types::TransformState,
        layers: &QueryLayers<'_>,
        options: &RenderedQueryOptions,
        global_state: Option<&GlobalStateMap>,
        proj_matrix: &Mat4,
        feature_state: &SourceFeatureState,
    ) -> Result<(), Error> {
        let layout = self.inner.layout.borrow().clone();
        // `if (!getData()) return;`
        let Some(layout) = layout else {
            return Ok(());
        };
        if layout.result.feature_index.get_data().is_none() {
            return Ok(());
        }

        let query_padding = self.get_query_padding(layers);

        let id = self.inner.base.id();
        let projector = mln_map_types::TileProjector::with_data(
            transform_state,
            &id.to_unwrapped(),
            transform_state.get_projection_data_with(&id.to_unwrapped(), proj_matrix),
        );

        layout.result.feature_index.query(
            result,
            query_geometry,
            transform_state,
            &projector,
            mln_util::constants::TILE_SIZE_D * f64::from(id.overscale_factor()),
            2f64.powf(transform_state.get_zoom() - f64::from(id.overscaled_z)),
            options,
            global_state,
            &id,
            layers,
            query_padding * transform_state.max_pitch_scale_factor(),
            feature_state,
        )
    }

    fn query_source_features(
        &self,
        result: &mut Vec<Feature>,
        options: &SourceQueryOptions,
        global_state: Option<&GlobalStateMap>,
    ) -> Result<(), Error> {
        let layout = self.inner.layout.borrow().clone();
        // Data not yet available, or tile is empty
        let Some(layout) = layout else {
            return Ok(());
        };
        let Some(data) = layout.result.feature_index.get_data() else {
            return Ok(());
        };

        // No source layers, specified, nothing to do
        let Some(source_layers) = &options.source_layers else {
            mln_util::logging::Log::warning(mln_util::logging::Event::General, "At least one sourceLayer required");
            return Ok(());
        };

        let id = self.inner.base.id();
        for source_layer in source_layers {
            // Go through all sourceLayers, if any
            // to gather all the features
            if let Some(layer) = data.layer(source_layer)? {
                let start = result.len();
                query_layer_source_features(&layer, result, options, global_state, &id)?;
                for feature in &mut result[start..] {
                    feature.source_layer.clone_from(source_layer);
                }
            }
        }
        Ok(())
    }
}

// query
use mln_util::feature::Feature;
use mln_util::mat4::Mat4;

use crate::geometry::feature_index::{QueryLayers, QueryResults};
use crate::query::{RenderedQueryOptions, SourceQueryOptions};
use crate::source_state::SourceFeatureState;

/// The feature loop of `GeometryTile::querySourceFeatures` and
/// `GeoJSONTile::querySourceFeatures`.
pub(crate) fn query_layer_source_features(
    layer: &mln_tile::TileLayer<'_>,
    result: &mut Vec<Feature>,
    options: &SourceQueryOptions,
    global_state: Option<&GlobalStateMap>,
    id: &OverscaledTileID,
) -> Result<(), Error> {
    for i in 0..layer.len() {
        let feature = layer.feature(i)?;

        // Apply filter, if any
        if let Some(filter) = &options.filter {
            let context =
                mln_style::expression::EvaluationContext::with_zoom_feature(f32::from(id.overscaled_z), Some(&feature))
                    .with_global_state(global_state);
            if !filter.evaluate(&context) {
                continue;
            }
        }

        let converted = mln_tile::convert_feature(&feature, &feature.geometry()?, &id.canonical)?;
        result.push(Feature::from(converted));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use mln_actor::scheduler::{Scheduler, Task};
    use mln_style_types::image::{ImageMap, Impl as ImageImpl};
    use mln_util::identity::SimpleIdentity;
    use mln_util::image::PremultipliedImage;
    use mln_util::size::Size;
    use parking_lot::Mutex;

    use super::*;

    /// A scheduler that runs its tasks only when it is waited for.
    #[derive(Default)]
    struct Held(Mutex<Vec<Task>>);

    impl Scheduler for Held {
        fn schedule(&self, task: Task) {
            self.0.lock().push(task);
        }

        fn wait_for_empty(&self) {
            let tasks = std::mem::take(&mut *self.0.lock());
            for task in tasks {
                task();
            }
        }
    }

    /// The last reference to a layout frees its feature index, and the tile
    /// data in it, on the workers, and gives its atlas bins back here.
    #[test]
    fn a_layout_frees_its_data_on_the_workers_and_its_atlases_here() {
        let workers = Arc::new(Held::default());
        let scheduler: mln_actor::scheduler::SchedulerPtr = Arc::<Held>::clone(&workers);
        let thread_pool = TaggedScheduler::new(scheduler, SimpleIdentity::new());
        let features = Arc::new(Vec::new());
        let data = Arc::downgrade(&features);
        let mut atlas = DynamicTextureAtlas::new();
        let image = Arc::new(
            ImageImpl::new(
                "icon".to_owned(),
                PremultipliedImage::new(Size::new(4, 4)),
                1.0,
                false,
                Vec::new(),
                Vec::new(),
                None,
                None,
                None,
            )
            .expect("image"),
        );
        let images = atlas.pack_images(&ImageMap::from([(image.id.clone(), image)]), &ImageMap::new());
        let glyphs = atlas.pack_glyphs(&mln_text::glyph::GlyphMap::new());
        let atlases = TileAtlases { glyphs: glyphs.lease, images: images.lease, pattern_positions: images.patterns };
        let result =
            LayoutResult::new(HashMap::default(), Arc::new(FeatureIndex::new(Some(TileData::Features(features)))));
        let layout = Rc::new(TileLayout::new(result, Some(atlases)));
        let render_data = Rc::clone(&layout);

        release_layout(layout, &thread_pool);
        atlas.free_released();
        assert_eq!(atlas.page_count(), 1, "the render data still holds the layout");
        release_layout(render_data, &thread_pool);
        atlas.free_released();
        assert_eq!(atlas.page_count(), 0, "the bins went back to the atlas");
        assert!(data.upgrade().is_some(), "the data waits for the workers");
        workers.wait_for_empty();
        assert!(data.upgrade().is_none(), "the workers freed the data");
    }
}
