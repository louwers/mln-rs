// The orchestrator owns the render sources and the tile store their tiles
// are in, and turns what the store reports into the renderer's events.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use mln_gfx::ShaderRegistry;
use mln_map_types::{MapMode, TransformState};
use mln_style::diff::{LayerChange, StyleDiff, sources_by_id};
use mln_style::expression::{Dependency, GlobalStateMap};
use mln_style::layer_type::{ErasedLayerStyle, SourceKind};
use mln_style::map::zoom_history::ZoomHistory;
use mln_style::{
    GlobalStateChange, Layer, PropertyEvaluationParameters, Source, Style, TransitionOptions, TransitionParameters,
    layer,
};
use mln_text::GlyphCache;
use mln_text::local_glyph_rasterizer::DefaultLocalGlyphRasterizer;
use mln_util::constants::DEFAULT_TRANSITION_DURATION;
use mln_util::error::Error;
use mln_util::hash::{HashMap, HashSet};
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use mln_util::pool::{Pool, TaskGroup};
use mln_util::time::Instant;
use mln_vec_map::{VecMap, VecSet};

use crate::Registry;
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::gfx::context::Context;
use crate::image_store::{ImageStore, ImageStoreObserver};
use crate::layer_group::{LayerGroup, LayerGroupPtr};
use crate::layer_tweaker::LayerTweaker;
use crate::layer_type::ErasedEvaluatedLayer;
use crate::line_atlas::LineAtlas;
use crate::paint_parameters::PaintParameters;
use crate::pattern_atlas::PatternAtlas;
use crate::render_layer::{LayerPrepareParameters, RenderLayerPtr};
use crate::render_source::{self, RenderSource, SourcePrepareParameters};
use crate::render_tree::{DebugType, LayerRenderItem, RenderItem, RenderTree, RenderTreeParameters};
use crate::renderer_event::{EventSink, RendererEvent, RendererEvents};
use crate::sources::render_tile_source::{PolylineLayerTweaker, debug_line_properties}; // layers: debug
use crate::text::glyph_store::{GlyphStore, GlyphStoreObserver};
use crate::text::placement::Placement;
use crate::tile::inbox::Waker;
use crate::tile::source_tiles::SourceTiles;
use crate::tile::store::{StoreEvent, TileStore};
use crate::tile::tile_workers::PoolWorkers;
use crate::tile::worker_codec::RemoteWorkers;
use crate::tile_parameters::TileParameters;
use crate::update_parameters::UpdateParameters;

/// The keys whose values differ between two global state snapshots. Values
/// of unchanged keys are copied verbatim between snapshots, so exact
/// equality is sufficient here.
fn diff_global_state_keys(previous: &GlobalStateMap, current: &GlobalStateMap) -> VecSet<String> {
    let mut keys = VecSet::new();
    for (key, value) in current {
        match previous.get(key) {
            Some(prev) if prev == value => {}
            _ => {
                keys.insert(key.clone());
            }
        }
    }
    for key in previous.keys() {
        if !current.contains_key(key) {
            keys.insert(key.clone());
        }
    }
    keys
}

/// The `ImageStoreObserver` half of `RenderOrchestrator`.
struct OrchestratorImageObserver {
    events: EventSink,
}

impl ImageStoreObserver for OrchestratorImageObserver {
    /// The map hears of the image and may add it to the style before the
    /// next update. If it doesn't, the tiles lay out without it, and again
    /// when it arrives.
    fn on_style_image_missing(&self, id: &str) {
        self.events.emit(RendererEvent::StyleImageMissing(id.to_owned()));
    }

    fn on_remove_unused_style_images(&self, ids: &[String]) {
        self.events.emit(RendererEvent::UnusedStyleImages(ids.to_vec()));
    }
}

/// symbols: the `GlyphStoreObserver` half of `RenderOrchestrator`.
struct OrchestratorGlyphObserver {
    events: EventSink,
}

impl GlyphStoreObserver for OrchestratorGlyphObserver {
    fn on_glyphs_error(
        &self,
        font_stack: &mln_util::font_stack::FontStack,
        glyph_range: mln_text::glyph::GlyphRange,
        error: Error,
    ) {
        let range = if glyph_range.type_ == mln_text::glyph::GlyphIDType::FontPBF {
            format!("{}-{}", glyph_range.first, glyph_range.second)
        } else {
            format!("{}(font file)", glyph_range.type_.id())
        };
        Log::error(
            Event::Style,
            format!(
                "Failed to load glyph range {range} for font stack {}:( {error})",
                mln_util::font_stack::font_stack_to_string(font_stack)
            ),
        );
        self.events.emit(RendererEvent::GlyphsError {
            font_stack: font_stack.clone(),
            range: glyph_range,
            error: error.clone(),
        });
        self.events.emit(RendererEvent::ResourceError(error));
    }

    fn on_glyphs_loaded(&self, font_stack: &mln_util::font_stack::FontStack, range: mln_text::glyph::GlyphRange) {
        self.events.emit(RendererEvent::GlyphsLoaded { font_stack: font_stack.clone(), range });
    }

    fn on_glyphs_requested(&self, font_stack: &mln_util::font_stack::FontStack, range: mln_text::glyph::GlyphRange) {
        self.events.emit(RendererEvent::GlyphsRequested { font_stack: font_stack.clone(), range });
    }
}

/// The jobs in flight per thread of the pool.
const JOBS_PER_THREAD: usize = 2;

/// The jobs in flight on a pool without threads, whose host runs them
/// between its own work.
const JOBS_WITHOUT_THREADS: usize = 8;

fn ptr_eq_option<T: ?Sized>(a: Option<&Arc<T>>, b: Option<&Arc<T>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

/// The render layer of the layer `style`, from its type in `registry`. A
/// type that the registry lacks breaks the contract that a map and its
/// renderer register alike; the layer isn't drawn.
fn create_render_layer(registry: &Registry, style: &Arc<dyn ErasedLayerStyle>) -> Option<RenderLayerPtr> {
    let name = style.info().name;
    let render_layer = registry.layer_type(name).and_then(|layer_type| layer_type.create_render_layer(style));
    debug_assert!(render_layer.is_some(), "the renderer's registry has no layer type '{name}'");
    if render_layer.is_none() {
        Log::error(Event::Render, format!("no render layer for layer type '{name}'"));
    }
    render_layer
}

pub(crate) struct RenderOrchestrator {
    registry: Arc<Registry>,
    events: EventSink,

    zoom_history: ZoomHistory,
    /// The still image request the last update was for.
    still_image_request_id: u64,
    transform_state: TransformState,

    /// The sources' tiles, with the glyph and image stores their layouts
    /// resolve through and the atlas they pack them into.
    tile_store: TileStore,
    line_atlas: Rc<RefCell<LineAtlas>>,
    pattern_atlas: Rc<RefCell<PatternAtlas>>,

    /// The version of the style the last update drew, or `None` before
    /// the first and after `clear_data`: no layers, sources or images.
    style: Option<Arc<Style>>,
    /// The style's sources in id order, the order of the per-source pass.
    sources_by_id: Vec<Source>,

    render_sources: HashMap<String, Box<dyn RenderSource>>,
    render_layers: HashMap<String, RenderLayerPtr>,
    // conformance: renderLight.
    render_light: crate::render_light::RenderLight,
    render_sky: Option<crate::render_sky::RenderSky>,

    // symbols:
    cross_tile_symbol_index: crate::text::cross_tile_symbol_index::CrossTileSymbolIndex,
    placement_controller: crate::text::placement::PlacementController,
    background_layer_as_color: bool,
    placed_symbol_data_collected: bool,
    /// Port-only: when the last `create_render_tree` started placing
    /// symbols, for the renderer's `RenderTimings`.
    pub(crate) symbol_placement_start: Option<Instant>,
    /// The center's longitude at the last update, which tells a wrap jump.
    longitude: f32,
    tile_cache_enabled: bool,

    /// Vector with reference wrappers to the layers in style order.
    ordered_layers: Vec<RenderLayerPtr>,
    /// Whether the next update makes the render layers again, for new
    /// shaders.
    remake_render_layers: bool,
    filtered_layers_for_source: Vec<Arc<dyn ErasedEvaluatedLayer>>,

    thread_pool: TaskGroup,

    global_state: Option<Arc<GlobalStateMap>>,

    /// Ordered by layer index, and by insertion order within an index.
    layer_groups_by_layer_index: VecMap<i32, Vec<LayerGroupPtr>>,

    /// The `layerTweakers` (weak pointers) each C++ `LayerGroupBase` holds,
    /// keyed by the layer group's id (see `change_request.rs`).
    layer_group_tweakers: HashMap<SimpleIdentity, Vec<Weak<RefCell<dyn LayerTweaker>>>>,

    /// The debug layer groups (layers: debug).
    debug_layer_groups: crate::render_tree::DebugLayerGroupMap,
    /// The tweaker of each debug layer group, with the group it was made
    /// for. The render items that fill the groups last one frame, so the
    /// orchestrator owns the tweakers (layers: debug).
    debug_layer_tweakers: VecMap<DebugType, (LayerGroupPtr, Rc<RefCell<PolylineLayerTweaker>>)>,

    // dem-heatmap: `std::vector<RenderTargetPtr> renderTargets`.
    render_targets: Vec<crate::render_target::RenderTargetPtr>,

    pending_changes: UniqueChangeRequestVec,

    globe_depth_pass: crate::globe_depth_pass::GlobeDepthPass,
    sky_pass: crate::sky_pass::SkyPass,

    /// Last, so that it drops after the fields above: the render layers,
    /// layer groups and placement hold render tiles, whose layouts free
    /// their data on the thread pool (`drop_on_workers`).
    _wait_for_thread_pool: WaitOnDrop,
}

impl RenderOrchestrator {
    /// An orchestrator whose background work runs in a group of its own on
    /// `pool`, which its drop waits for, and whose tiles and glyphs call
    /// `waker` for an update (see `Renderer::new`).
    pub(crate) fn new(
        background_layer_as_color: bool,
        local_font_family: Option<String>,
        glyph_cache: Arc<GlyphCache>,
        font_face_engine: Arc<dyn crate::text::font_face_engine::FontFaceEngine>,
        registry: Arc<Registry>,
        waker: Waker,
        pool: &Pool,
    ) -> Self {
        let events = EventSink::default();
        // `imageManager->setObserver(this)`.
        let mut image_store = ImageStore::default();
        image_store.set_observer(Some(Rc::new(OrchestratorImageObserver { events: events.clone() })));
        // symbols: `glyphManager->setObserver(this)`.
        let thread_pool = TaskGroup::new(pool);
        let mut glyph_store = GlyphStore::new(
            Some(Box::new(DefaultLocalGlyphRasterizer::new(local_font_family))),
            glyph_cache,
            thread_pool.clone(),
            font_face_engine,
            Arc::clone(&waker),
        );
        glyph_store.set_observer(Some(Rc::new(OrchestratorGlyphObserver { events: events.clone() })));
        let jobs = match pool.threads() {
            0 => JOBS_WITHOUT_THREADS,
            threads => JOBS_PER_THREAD * threads,
        };
        let pool_workers = PoolWorkers::new(thread_pool.clone(), jobs);
        let tile_store = TileStore::new(Box::new(pool_workers), glyph_store, image_store, thread_pool.clone(), waker);
        RenderOrchestrator {
            registry,
            events,
            zoom_history: ZoomHistory::default(),
            still_image_request_id: 0,
            transform_state: TransformState::default(),
            tile_store,
            cross_tile_symbol_index: crate::text::cross_tile_symbol_index::CrossTileSymbolIndex::default(),
            placement_controller: crate::text::placement::PlacementController::new(),
            line_atlas: Rc::new(RefCell::new(LineAtlas::new())),
            pattern_atlas: Rc::new(RefCell::new(PatternAtlas::new())),
            style: None,
            sources_by_id: Vec::new(),
            render_sources: HashMap::default(),
            render_layers: HashMap::default(),
            // conformance: renderLight(makeMutable<Light::Impl>())
            render_light: crate::render_light::RenderLight::new(Arc::default()),
            render_sky: None,
            background_layer_as_color,
            placed_symbol_data_collected: false,
            symbol_placement_start: None,
            longitude: 0.0,
            tile_cache_enabled: true,
            ordered_layers: Vec::new(),
            remake_render_layers: false,
            filtered_layers_for_source: Vec::new(),
            _wait_for_thread_pool: WaitOnDrop(thread_pool.clone()),
            thread_pool,
            global_state: None,
            layer_groups_by_layer_index: VecMap::new(),
            layer_group_tweakers: HashMap::default(),
            globe_depth_pass: crate::globe_depth_pass::GlobeDepthPass::default(),
            sky_pass: crate::sky_pass::SkyPass::default(),
            debug_layer_groups: VecMap::new(),   // layers: debug
            debug_layer_tweakers: VecMap::new(), // layers: debug
            render_targets: Vec::new(),          // dem-heatmap
            pending_changes: Vec::new(),
        }
    }

    /// Port-only: runs the tiles' jobs in `workers` (by default on the
    /// thread pool). The jobs in flight elsewhere run again there.
    pub(crate) fn set_tile_workers(&mut self, workers: RemoteWorkers) {
        self.tile_store.set_workers(Box::new(workers));
    }

    /// Sets the queue the orchestrator and its parts push their events to.
    pub(crate) fn set_event_queue(&mut self, events: Option<RendererEvents>) {
        self.events.set(events);
    }

    pub(crate) fn emit(&self, event: RendererEvent) {
        self.events.emit(event);
    }

    pub(crate) fn get_render_source(&self, id: &str) -> Option<&dyn RenderSource> {
        self.render_sources.get(id).map(AsRef::as_ref)
    }

    /// The orchestrator's line atlas.
    pub(crate) fn get_line_atlas(&self) -> &Rc<RefCell<LineAtlas>> {
        &self.line_atlas
    }

    pub(crate) fn get_pattern_atlas(&self) -> &Rc<RefCell<PatternAtlas>> {
        &self.pattern_atlas
    }

    pub(crate) fn create_render_tree(&mut self, update_parameters: &Arc<UpdateParameters>) -> Option<RenderTree> {
        let start_time = Instant::now();

        let is_map_mode_continuous = update_parameters.mode == MapMode::Continuous;
        if !is_map_mode_continuous {
            // Reset zoom history state.
            self.zoom_history.first = true;
        }

        let zoom_changed =
            self.zoom_history.update(update_parameters.transform_state.get_zoom() as f32, update_parameters.time_point);

        let style = &update_parameters.style;
        let transition_options =
            if is_map_mode_continuous { style.get_transition_options() } else { TransitionOptions::default() };

        let transition_duration = transition_options.duration.unwrap_or(if is_map_mode_continuous {
            DEFAULT_TRANSITION_DURATION
        } else {
            Duration::ZERO
        });

        let new_global_state = Some(Arc::clone(style.get_global_state()));
        let global_state_changed = !ptr_eq_option(self.global_state.as_ref(), new_global_state.as_ref());
        let mut changed_global_state_keys: Option<Arc<VecSet<String>>> = None;
        let mut global_state_change = None;
        if global_state_changed && let (Some(previous), Some(current)) = (&self.global_state, &new_global_state) {
            let changed_keys = Arc::new(diff_global_state_keys(previous, current));
            changed_global_state_keys = Some(Arc::clone(&changed_keys));
            global_state_change =
                Some(GlobalStateChange { prior_state: Arc::clone(previous), changed_keys, state_only: false });
        }
        self.global_state = new_global_state;

        // A layer added this frame has shown nothing yet, so a global-state
        // change leaves it nothing to transition from.
        let added_layer_transition_parameters =
            TransitionParameters::new(update_parameters.time_point, transition_options);
        let transition_parameters = TransitionParameters { global_state_change, ..added_layer_transition_parameters };

        // Transitions the values that read a changed global-state key, where
        // nothing else changed.
        let global_state_transition_parameters =
            transition_parameters.global_state_change.as_ref().map(|change| TransitionParameters {
                global_state_change: Some(GlobalStateChange { state_only: true, ..change.clone() }),
                ..transition_parameters.clone()
            });

        let mut evaluation_parameters = PropertyEvaluationParameters::with_zoom_history(
            self.zoom_history,
            update_parameters.time_point,
            transition_duration,
        );
        evaluation_parameters.zoom_changed = zoom_changed;
        evaluation_parameters.global_state = self.global_state.clone();
        evaluation_parameters.global_state_changed = global_state_changed;
        evaluation_parameters.changed_global_state_keys = changed_global_state_keys.clone();

        let mut tile_parameters = TileParameters {
            pixel_ratio: update_parameters.pixel_ratio,
            debug_options: update_parameters.debug_options,
            transform_state: &update_parameters.transform_state,
            file_source: update_parameters.file_source.clone(),
            mode: update_parameters.mode,
            prefetch_zoom_delta: update_parameters.prefetch_zoom_delta,
            tile_lod_min_radius: update_parameters.tile_lod_min_radius,
            tile_lod_scale: update_parameters.tile_lod_scale,
            tile_lod_pitch_threshold: update_parameters.tile_lod_pitch_threshold,
            tile_lod_zoom_shift: update_parameters.tile_lod_zoom_shift,
            tile_lod_mode: update_parameters.tile_lod_mode,
            subdivision_granularity: update_parameters.subdivision_granularity,
            global_state: self.global_state.clone(),
            is_update_synchronous: false,
            font_faces: style.get_font_faces().cloned(),
        };

        self.tile_store.glyphs().set_url(style.get_glyph_url());
        self.tile_store.glyphs().set_font_faces(style.get_font_faces().cloned());
        self.tile_store.set_file_source(update_parameters.file_source.clone());

        // What arrived for the tiles before the sources update, so that a
        // tile laid out since the last update renders in this one.
        self.receive_tiles();

        // Update light. (conformance)
        let light_changed = !Arc::ptr_eq(&self.render_light.light, style.get_light());

        if light_changed {
            self.render_light.light = Arc::clone(style.get_light());
            self.render_light.transition(&transition_parameters);
        } else if let Some(parameters) = &global_state_transition_parameters {
            self.render_light.transition(parameters);
        }

        if light_changed || zoom_changed || global_state_changed || self.render_light.has_transition() {
            self.render_light.evaluate(&evaluation_parameters);
        }

        // Update sky while preserving transitions between successive configurations.
        let mut sky_changed = false;
        if let Some(sky) = style.get_sky() {
            match &mut self.render_sky {
                None => {
                    self.render_sky = Some(crate::render_sky::RenderSky::new(Arc::clone(sky)));
                    sky_changed = true;
                }
                Some(render_sky) if !Arc::ptr_eq(&render_sky.sky, sky) => {
                    render_sky.sky = Arc::clone(sky);
                    render_sky.transition(&transition_parameters);
                    sky_changed = true;
                }
                Some(render_sky) => {
                    if let Some(parameters) = &global_state_transition_parameters {
                        render_sky.transition(parameters);
                    }
                }
            }
        } else if self.render_sky.is_some() {
            self.render_sky = None;
            sky_changed = true;
        }

        if let Some(render_sky) = &mut self.render_sky
            && (sky_changed || zoom_changed || global_state_changed || render_sky.has_transition())
        {
            render_sky.evaluate(&evaluation_parameters);
        }

        // The previous sources may hold the last references to replaced
        // data, such as a GeoJSON source's index, which can take
        // milliseconds to free. The workers drop them once this update has
        // moved the render sources and their tiles to the new ones. Declared
        // before the diff, which holds previous sources too, so that it
        // drops after the diff. The previous version itself drops here, on
        // the render thread, with its layers and images.
        let previous_sources = self.style.as_ref().map(|previous| previous.get_sources());
        let sources_changed = previous_sources.is_none_or(|previous| !Arc::ptr_eq(previous, style.get_sources()));
        let _previous_sources = previous_sources
            .filter(|_| sources_changed)
            .map(|previous| DropOnWorkers::new(Arc::clone(previous), self.thread_pool.clone()));
        let StyleDiff { layers: layer_changes, sources: source_diff, images: image_diff } =
            StyleDiff::between(self.style.as_deref(), style);
        if sources_changed {
            self.sources_by_id = sources_by_id(style.get_sources());
        }
        self.style = Some(Arc::clone(style));
        let layers = Arc::clone(style.get_layer_list());

        // Adding or removing an image changes the layout of the tiles whose
        // dependencies name it, and updating one does when its size or SDF
        // flag changes. An update of the same size patches its atlas bins.
        let images = self.tile_store.images_mut();
        images.begin_update();
        let mut relayout_images: HashSet<String> = HashSet::default();
        for id in image_diff.removed.keys() {
            images.remove_image(id);
            self.pattern_atlas.borrow_mut().remove_pattern(id);
            relayout_images.insert(id.clone());
        }
        for image in image_diff.added.values() {
            images.add_image(Arc::clone(image));
            relayout_images.insert(image.id.clone());
        }
        let mut patched = Vec::new();
        for image in image_diff.changed.values() {
            if images.update_image(Arc::clone(image)) {
                self.pattern_atlas.borrow_mut().remove_pattern(&image.id);
                relayout_images.insert(image.id.clone());
            }
            patched.push(Arc::clone(image));
        }
        images.set_loaded(update_parameters.sprite_loaded);
        for image in patched {
            self.tile_store.atlas().patch_image(&image);
        }
        if !relayout_images.is_empty() {
            self.tile_store.images_changed(&relayout_images);
        }

        let mut changes: UniqueChangeRequestVec = Vec::new();

        // Make each render layer again from its layer, without the relayout
        // that adding the layer would take.
        let render_layers_remade = std::mem::take(&mut self.remake_render_layers);
        if render_layers_remade {
            for render_layer in self.render_layers.values_mut() {
                let impl_ = Arc::clone(&render_layer.borrow().base().style);
                let Some(new_layer) = create_render_layer(&self.registry, &impl_) else {
                    continue;
                };
                render_layer.borrow_mut().layer_removed(&mut changes);
                new_layer.borrow_mut().transition(&added_layer_transition_parameters);
                *render_layer = new_layer;
            }
        }
        let layers_added_or_removed = render_layers_remade || layer_changes.order_changed;

        // Remove render layers for removed layers.
        for (id, _) in layer_changes.iter().filter(|(_, change)| change.removes()) {
            if let Some(layer) = self.render_layers.remove(&**id) {
                layer.borrow_mut().layer_removed(&mut changes);
            }
        }

        // Create render layers for newly added layers.
        for (id, _) in layer_changes.iter().filter(|(_, change)| change.adds()) {
            let Some(layer) = layers.get(id) else { continue };
            if let Some(render_layer) = create_render_layer(&self.registry, layer.style()) {
                render_layer.borrow_mut().transition(&added_layer_transition_parameters);
                // `emplace` keeps an existing entry.
                self.render_layers.entry(id.to_string()).or_insert(render_layer);
            }
        }

        // Update render layers for changed layers.
        for (id, _) in layer_changes.iter().filter(|(_, change)| change.restyles()) {
            if let (Some(render_layer), Some(layer)) = (self.render_layers.get(&**id), layers.get(id)) {
                let new_layer = layer.style();

                let mut render_layer = render_layer.borrow_mut();
                render_layer.layer_changed(&transition_parameters, new_layer, &mut changes);
                render_layer.restyle(&transition_parameters, new_layer);
            }
        }

        if layers_added_or_removed {
            self.ordered_layers.clear();
            self.ordered_layers.reserve(layers.len());
            let mut layer_index: i32 = 0;
            for style_layer in layers.iter() {
                // C++ uses renderLayers.at(id); a layer type without a
                // render layer is skipped.
                let Some(layer) = self.render_layers.get(style_layer.get_id()) else {
                    continue;
                };
                self.ordered_layers.push(Rc::clone(layer));

                // We're mutating the list of ordered layers and must notify
                // them of their new assigned indices
                layer.borrow_mut().layer_index_changed(layer_index, &mut changes);
                layer_index += 1;
            }
        }
        debug_assert_eq!(self.ordered_layers.len(), self.render_layers.len());

        if layers_added_or_removed || !layer_changes.is_empty() {
            self.tile_store.glyphs().evict(&layer::font_stacks(layers.iter().map(Layer::style)));
        }

        // Update layers for class and zoom changes.
        let mut constants_mask_changed: VecSet<String> = VecSet::new();
        for layer in &self.ordered_layers {
            let mut layer = layer.borrow_mut();
            let id = layer.get_id().to_owned();
            let layer_added_or_changed =
                render_layers_remade || layer_changes.get(&id).is_some_and(|change| change != LayerChange::Removed);
            evaluation_parameters.layer_changed = layer_added_or_changed;
            evaluation_parameters.has_crossfade = layer.has_crossfade();

            // Only re-evaluate on change of zoom if the style has some
            // reference to it
            let zoom_changed_and_matters =
                zoom_changed && !layer_added_or_changed && layer.get_style_dependencies().intersects(Dependency::ZOOM);

            // Similarly, only re-evaluate on a global state change if the
            // layer references the global state.
            let global_state_changed_and_matters = global_state_changed
                && !layer_added_or_changed
                && layer.get_style_dependencies().intersects(Dependency::GLOBAL_STATE);

            // A running transition may read the changed state too, from a
            // value the layer no longer has.
            if let Some(parameters) = &global_state_transition_parameters
                && !layer_added_or_changed
                && (global_state_changed_and_matters || layer.has_transition())
            {
                layer.transition(parameters);
            }

            if layer_added_or_changed
                || zoom_changed_and_matters
                || global_state_changed_and_matters
                || evaluation_parameters.has_crossfade
                || layer.has_transition()
            {
                let previous_mask = layer.base().evaluated_properties.constants_mask();
                layer.evaluate(&evaluation_parameters);
                if previous_mask != layer.base().evaluated_properties.constants_mask() {
                    constants_mask_changed.insert(id);
                }
            }
        }

        // Remove render layers for removed sources.
        for id in source_diff.removed.keys() {
            if let Some(key) = self.render_sources.remove(id).and_then(|source| source.tiles()) {
                self.tile_store.remove_source(key);
            }
        }

        // Create render sources for newly added sources.
        for (id, style_source) in &source_diff.added {
            if self.render_sources.contains_key(id) {
                // `emplace` keeps an existing entry.
                continue;
            }
            let Some(mut render_source) = render_source::create(&self.registry, style_source, &mut self.tile_store)
            else {
                continue;
            };
            render_source.set_fast_pfor_enabled(update_parameters.fast_pfor_enabled);
            self.render_sources.insert(id.clone(), render_source);
        }

        // The update carries the map's whole feature state, so a render
        // source, whether new, kept or rebuilt after `clear_data`, shows the
        // state of the update it renders. A source whose state the update
        // didn't change does nothing.
        for (id, render_source) in &self.render_sources {
            if let Some((source, _)) = render_source.tiles().and_then(|key| self.tile_store.source_mut(key)) {
                source.info.feature_state.set_states(update_parameters.feature_states.get(id));
            }
        }

        // A still image fails at the first resource error, and nothing
        // requests a failed resource again on its own: each new still image
        // retries them instead of waiting on them or rendering without them.
        if !is_map_mode_continuous
            && update_parameters.still_image_request
            && update_parameters.still_image_request_id != self.still_image_request_id
        {
            self.still_image_request_id = update_parameters.still_image_request_id;
            // A tile laid out without the glyphs of a failed range lays out
            // again, which requests the range again.
            self.tile_store.retry_failed();
        }
        self.transform_state = update_parameters.transform_state.clone();
        let _tilted_view = self.transform_state.get_pitch() != 0.0;

        // Create parameters for the render tree.
        let mut render_tree_parameters =
            Box::new(RenderTreeParameters::new(&update_parameters.transform_state, update_parameters.debug_options));
        // conformance: the RenderTreeParameters ctor's `renderLight.getEvaluated()`.
        render_tree_parameters.light = self.render_light.get_evaluated().clone();
        render_tree_parameters.sky = self.render_sky.as_ref().map(|render_sky| render_sky.get_evaluated().clone());

        let mut layer_render_items: VecMap<u32, LayerRenderItem> = VecMap::new();

        // Reserve size for filteredLayersForSource if there are sources.
        if !self.sources_by_id.is_empty() {
            self.filtered_layers_for_source.reserve(layers.len());
        }

        // Track which layers are flagged for rendering
        let mut update_list: Vec<bool> = vec![false; self.ordered_layers.len()];

        // Update all sources and initialize renderItems.
        //
        // DEVIATION: C++ always has at least one source, the annotation
        // source that AnnotationManager::onStyleLoaded adds to every style,
        // and handles the layers without a source (background) while it
        // iterates over the first source. Annotations are not ported yet, so
        // with no style sources the loop runs once without a source.
        let source_count = std::cmp::max(self.sources_by_id.len(), 1);
        for source_index in 0..source_count {
            let style_source = self.sources_by_id.get(source_index).cloned();
            let mut source_needs_rendering = false;
            let mut source_needs_relayout = false;
            // C++ has a render source for every source type; the port skips
            // the layers of a source type without one.
            let has_render_source =
                style_source.as_ref().is_some_and(|style_source| self.render_sources.contains_key(style_source.id()));
            // The id of the source whose layers this pass takes, if it has a
            // render source.
            let render_source_id = style_source.as_ref().filter(|_| has_render_source).map(Source::id);

            for (index, layer_ptr) in self.ordered_layers.iter().enumerate() {
                let layer = layer_ptr.borrow();
                let style = &layer.base().style;
                let layer_info = style.info();
                render_tree_parameters.has_3d |= layer_info.pass_3d;

                if layer_info.source != SourceKind::None {
                    // Every source's pass sees every layer, so the cheap
                    // source check goes before anything else is read.
                    if let Some(style_source) = &style_source
                        && render_source_id == Some(style.base().source.as_str())
                    {
                        let layer_is_visible = style.base().visibility != mln_style_types::VisibilityType::None;
                        let zoom_fits_layer = layer.supports_zoom(self.zoom_history.last_zoom);
                        let layer_id = layer.get_id().to_owned();
                        // A global state change requires a relayout of
                        // this layer's source if one of the changed state
                        // properties is referenced by anything that is
                        // baked into the tile's buckets.
                        let global_state_requires_relayout = {
                            if !global_state_changed
                                || !(style.layout_dependencies()
                                    | layer.base().evaluated_properties.evaluated_dependencies())
                                .intersects(Dependency::GLOBAL_STATE)
                            {
                                false
                            } else if changed_global_state_keys.is_none() {
                                true
                            } else {
                                let mut refs = VecSet::new();
                                style.collect_layout_global_state_refs(&mut refs);
                                layer.base().evaluated_properties.collect_evaluated_global_state_refs(&mut refs);
                                mln_style::expression::global_state_refs_intersect(
                                    Some(&refs),
                                    changed_global_state_keys.as_deref(),
                                )
                            }
                        };
                        source_needs_relayout = source_needs_relayout
                            || constants_mask_changed.contains(&layer_id)
                            || layer_changes.get(&layer_id).is_some_and(LayerChange::relayouts)
                            || global_state_requires_relayout;
                        if layer_is_visible {
                            self.filtered_layers_for_source.push(Arc::clone(&layer.base().evaluated_properties));
                            if zoom_fits_layer {
                                source_needs_rendering = true;
                                let source_id = style_source.id().to_owned();
                                layer_render_items
                                    .entry(index as u32)
                                    .or_insert_with(|| LayerRenderItem::new(Rc::clone(layer_ptr), Some(source_id)));
                                update_list[index] = true;
                            }
                        }
                    }
                    continue;
                }

                // Handle layers without source.
                let is_first_source = source_index == 0;
                if is_first_source
                    && style.base().visibility != mln_style_types::VisibilityType::None
                    && layer.supports_zoom(self.zoom_history.last_zoom)
                {
                    // On the globe the background is a sphere mesh, not the
                    // backdrop, so it stays a layer.
                    if self.background_layer_as_color
                        && render_tree_parameters.sky.is_none()
                        && !update_parameters.transform_state.is_globe_rendering()
                        && layers.first().is_some_and(|first| Arc::ptr_eq(style, first.style()))
                    {
                        let solid_background = layer.get_solid_background();
                        if let Some(solid_background) = solid_background {
                            render_tree_parameters.background_color = solid_background;
                            continue; // This layer is shown with background color,
                            // and it shall not be added to render items.
                        }
                    }
                    layer_render_items
                        .entry(index as u32)
                        .or_insert_with(|| LayerRenderItem::new(Rc::clone(layer_ptr), None));
                    update_list[index] = true;
                }
            }
            if let Some(style_source) = &style_source {
                tile_parameters.is_update_synchronous = style_source.data().is_update_synchronous();
                let filtered_layers = std::mem::take(&mut self.filtered_layers_for_source);
                // C++ `renderSources.at(sourceImpl->id)`; a source type
                // without a render source has none.
                if let Some(source) = self.render_sources.get_mut(style_source.id()) {
                    source.update(
                        &mut self.tile_store,
                        style_source.clone(),
                        &filtered_layers,
                        source_needs_rendering,
                        source_needs_relayout,
                        &tile_parameters,
                    );
                }
                self.filtered_layers_for_source = filtered_layers;
                self.filtered_layers_for_source.clear();
            }
        }

        // A synchronous source's tile laid out by its answer isn't in this
        // update's render tiles yet: the next update has it.
        let answered = self.tile_store.drive();
        self.forward_store_events();
        if answered {
            self.events.emit(RendererEvent::Invalidate);
        }

        // The sources' tiles are in other world copies once the center has
        // crossed the antimeridian (`SourceTiles::handle_wrap_jump`), and
        // the layers' drawables go with them.
        let longitude =
            update_parameters.transform_state.get_lat_lng(mln_util::geo::WrapMode::Unwrapped).longitude() as f32;
        let wrap_delta = ((longitude - self.longitude) / 360.0).round() as i16;
        self.longitude = longitude;

        // Update all layers with their new renderability status, if it
        // changed.
        for (i, will_render) in update_list.iter().enumerate() {
            let mut layer = self.ordered_layers[i].borrow_mut();
            if wrap_delta != 0 {
                layer.handle_wrap_jump(wrap_delta);
            }
            if layer.is_layer_renderable() != *will_render {
                layer.mark_layer_renderable(*will_render, &mut changes);
            }
        }
        self.add_changes(&mut changes);

        render_tree_parameters.loaded = update_parameters.style_loaded && !answered && self.is_loaded();
        if !is_map_mode_continuous && !render_tree_parameters.loaded {
            return None;
        }

        // Prepare. Update all matrices and generate data that we should upload
        // to the GPU.
        for (name, render_source) in &mut self.render_sources {
            if render_source.is_enabled() {
                render_source.prepare(
                    &mut self.tile_store,
                    &SourcePrepareParameters {
                        transform: &render_tree_parameters.transform_params,
                        debug_options: update_parameters.debug_options,
                        source_name: name.clone(),
                    },
                );
            } else {
                render_source.release_render_data();
            }
        }

        let mut layers_need_placement: Vec<RenderLayerPtr> = Vec::new();
        let mut layers_following_placement: Vec<RenderLayerPtr> = Vec::new();
        let mut opaque_pass_cut_off_estimation = layer_render_items.len();
        for render_item in layer_render_items.values() {
            let mut render_layer = render_item.layer.borrow_mut();
            let source = render_item.source.as_ref().and_then(|id| self.render_sources.get(id)).map(AsRef::as_ref);
            render_layer.prepare(&mut LayerPrepareParameters {
                source,
                images: self.tile_store.images(), // conformance
                pattern_atlas: &self.pattern_atlas,
                line_atlas: &self.line_atlas,
            });
            if render_layer.needs_placement() {
                layers_need_placement.push(Rc::clone(&render_item.layer));
            }
            if !render_layer.get_follower_data().is_empty() {
                layers_following_placement.push(Rc::clone(&render_item.layer));
            }
            if render_tree_parameters.opaque_pass_cut_off == 0 {
                opaque_pass_cut_off_estimation -= 1;
                if render_layer.is_3d() {
                    render_tree_parameters.opaque_pass_cut_off = opaque_pass_cut_off_estimation as u32;
                }
            }
        }

        // Symbol placement. (symbols)
        self.symbol_placement_start = Some(Instant::now());
        let mut symbol_buckets_changed = false;
        let mut symbol_buckets_added = false;
        let mut used_symbol_layers: VecSet<String> = VecSet::new();
        let longitude =
            update_parameters.transform_state.get_lat_lng(mln_util::geo::WrapMode::Unwrapped).longitude() as f32;
        for layer in layers_need_placement.iter().rev() {
            let layer = layer.borrow();
            let result = self.cross_tile_symbol_index.add_layer(&*layer, longitude, &update_parameters.transform_state);
            if is_map_mode_continuous {
                used_symbol_layers.insert(layer.get_id().to_owned());
                symbol_buckets_added |= result.buckets_added;
                symbol_buckets_changed |= result.buckets_added || result.buckets_removed;
            }
        }

        if is_map_mode_continuous && layers_need_placement.is_empty() {
            // Discard deferred placement and query data when no symbol buckets remain.
            render_tree_parameters.placement_changed = self.placement_controller.reset();
            self.cross_tile_symbol_index.reset();
            // Source fades still need their placement bookkeeping to advance.
            self.tile_store.update_fading_tiles();
            render_tree_parameters.symbol_fade_change = 1.0;
            render_tree_parameters.needs_repaint = self.has_transitions(update_parameters.time_point);
        } else if is_map_mode_continuous {
            let tilted_view = update_parameters.transform_state.get_pitch() != 0.0;
            let mut placement_update_period_override: Option<Duration> = None;
            if symbol_buckets_added && !tilted_view {
                // If the view is not tilted, we want *the new* symbols to
                // show up faster, however simple setting `placementChanged`
                // to `true` would initiate placement too often as new
                // buckets usually come from several rendered tiles in a row
                // within a short period of time. Instead, we squeeze
                // placement update period to coalesce buckets updates from
                // several tiles. On contrary, with the tilted view it's more
                // important to make placement rarely for performance reasons
                // and as the new symbols are normally "far away" and the
                // user is not that interested to see them ASAP.
                placement_update_period_override = Some(Duration::from_millis(30));
            }

            render_tree_parameters.placement_changed = !self.placement_controller.placement_is_recent(
                update_parameters.time_point,
                update_parameters.transform_state.get_zoom() as f32,
                placement_update_period_override,
            );
            symbol_buckets_changed |= render_tree_parameters.placement_changed;
            if render_tree_parameters.placement_changed {
                let prev = self.placement_controller.take_placement();
                let mut placement = Placement::create(Arc::clone(update_parameters), Some(prev));
                placement.place_layers(&layers_need_placement);
                self.placement_controller.set_placement(placement);
                self.cross_tile_symbol_index.prune_unused_layers(&used_symbol_layers);
                self.tile_store.update_fading_tiles();
            } else {
                self.placement_controller.set_placement_stale();
            }
            render_tree_parameters.symbol_fade_change =
                self.placement_controller.get_placement().symbol_fade_change(update_parameters.time_point);
            render_tree_parameters.needs_repaint = self.has_transitions(update_parameters.time_point);
        } else {
            render_tree_parameters.placement_changed = !layers_need_placement.is_empty();
            symbol_buckets_changed = render_tree_parameters.placement_changed;
            if render_tree_parameters.placement_changed {
                let mut placement = Placement::create(Arc::clone(update_parameters), None);
                placement.collect_placed_symbol_data(self.placed_symbol_data_collected);
                placement.place_layers(&layers_need_placement);
                self.placement_controller.set_placement(placement);
            }
            self.cross_tile_symbol_index.reset();
            render_tree_parameters.symbol_fade_change = 1.0;
            render_tree_parameters.needs_repaint = false;
        }

        if !render_tree_parameters.needs_repaint && render_tree_parameters.loaded {
            // Notify observer about unused images when map is fully loaded
            // and there are no ongoing transitions.
            if self.tile_store.images().exceeds_cache_size_limit() {
                self.tile_store.images().reduce_memory_use(&self.tile_store.images_used());
            }
        }

        let mut source_render_items: Vec<Box<dyn RenderItem>> = Vec::new();
        for render_source in self.render_sources.values_mut() {
            if render_source.is_enabled() {
                source_render_items.push(render_source.create_render_item());
            }
        }

        let mut render_tree =
            RenderTree::new(render_tree_parameters, layer_render_items, source_render_items, start_time);
        // symbols: layersNeedPlacement, layersFollowingPlacement, placement,
        // updateSymbolOpacities.
        render_tree.set_placement(layers_need_placement, layers_following_placement, symbol_buckets_changed);
        Some(render_tree)
    }

    /// The current symbol placement.
    pub(crate) fn placement(&self) -> &Placement {
        self.placement_controller.get_placement()
    }

    pub(crate) fn has_transitions(&self, time_point: Option<Instant>) -> bool {
        // conformance
        if self.render_light.has_transition() {
            return true;
        }

        if self.render_sky.as_ref().is_some_and(crate::render_sky::RenderSky::has_transition) {
            return true;
        }

        for layer in self.render_layers.values() {
            if layer.borrow().has_transition() {
                return true;
            }
        }

        if self.placement_controller.has_transitions(time_point) {
            return true;
        }

        self.tile_store.has_fading_tiles()
    }

    pub(crate) fn is_loaded(&self) -> bool {
        // do the simple boolean check before iterating over all the tiles in
        // all the sources
        if !self.tile_store.images().is_loaded() {
            return false;
        }

        for source in self.render_sources.values() {
            if !source.is_loaded(&self.tile_store) {
                return false;
            }
        }

        true
    }

    pub(crate) fn clear_data(&mut self) {
        // The next update finds every layer, source and image added. The
        // global state, light and sky stay, so the next update doesn't
        // transition them again.
        self.style = None;
        self.sources_by_id.clear();

        let mut changes: UniqueChangeRequestVec = Vec::new();
        for layer in self.render_layers.values() {
            layer.borrow_mut().layer_removed(&mut changes);
        }
        self.add_changes(&mut changes);

        self.debug_layer_groups.clear(); // layers: debug
        self.debug_layer_tweakers.clear(); // layers: debug
        self.render_sources.clear();
        self.tile_store.clear();
        self.render_layers.clear();
        // C++ keeps orderedLayers here and rebuilds it on the next
        // createRenderTree (the layer diff then reports additions).
        self.ordered_layers.clear();

        self.cross_tile_symbol_index.reset();
        if !self.line_atlas.borrow().is_empty() {
            self.line_atlas = Rc::new(RefCell::new(LineAtlas::new()));
        }

        if !self.pattern_atlas.borrow().is_empty() {
            self.pattern_atlas = Rc::new(RefCell::new(PatternAtlas::new()));
        }

        self.tile_store.images_mut().clear();
        self.tile_store.glyphs().evict(&VecSet::new());
    }

    /// Drops what holds the renderer's shaders: the render layers, the sky
    /// and globe depth passes, and the debug drawables. The next update
    /// makes them again, from the shaders the renderer has then. The
    /// sources keep their tiles, and the tiles their layout.
    pub(crate) fn release_shaders(&mut self) {
        self.remake_render_layers = true;
        self.debug_layer_groups.clear(); // layers: debug
        self.debug_layer_tweakers.clear(); // layers: debug
        self.sky_pass = crate::sky_pass::SkyPass::default();
        self.globe_depth_pass = crate::globe_depth_pass::GlobeDepthPass::default();
    }

    /// Makes the atlas's GPU textures and uploads what its pages queued.
    pub(crate) fn upload_atlas(&mut self, context: &Context) {
        self.tile_store.atlas().upload(context);
    }

    /// Takes in what arrived for the tiles, and starts the jobs that it
    /// makes ready.
    pub(crate) fn receive_tiles(&mut self) {
        self.tile_store.receive();
        self.tile_store.drive();
        self.forward_store_events();
    }

    /// Turns what the tile store reported into the renderer's events.
    fn forward_store_events(&mut self) {
        for event in self.tile_store.take_events() {
            match event {
                StoreEvent::Changed => self.events.emit(RendererEvent::Invalidate),
                StoreEvent::Error { source, tile, error } => {
                    Log::error(Event::Style, format!("Failed to load tile {tile} for source {source}: {error}"));
                    self.events.emit(RendererEvent::ResourceError(error));
                    // A failed tile is done loading, and its parents or
                    // children may stand in for it, as when it changes: a
                    // continuous map needs a frame to see that, or it never
                    // reports that it finished loading. The error goes
                    // first, so the still image it fails isn't rendered
                    // again.
                    self.events.emit(RendererEvent::Invalidate);
                }
                StoreEvent::Action { op, tile, source } => {
                    self.events.emit(RendererEvent::TileAction { op, id: tile, source_id: source });
                }
            }
        }
    }

    /// The tiles of source `id`, if it has any.
    fn source_tiles(&self, id: &str) -> Option<&SourceTiles> {
        self.tile_store.source(self.render_sources.get(id)?.tiles()?)
    }

    fn source_tiles_mut(&mut self, id: &str) -> Option<&mut SourceTiles> {
        let key = self.render_sources.get(id)?.tiles()?;
        Some(self.tile_store.source_mut(key)?.0)
    }

    fn add_changes(&mut self, changes: &mut UniqueChangeRequestVec) {
        self.pending_changes.append(changes);
    }

    pub(crate) fn update_layer_index(&mut self, layer_group: &LayerGroupPtr, new_index: i32) {
        let current = layer_group.borrow().layer_index;
        if current == new_index {
            return;
        }
        if let Some(groups) = self.layer_groups_by_layer_index.get_mut(&current)
            && let Some(pos) = groups.iter().position(|g| Rc::ptr_eq(g, layer_group))
        {
            groups.remove(pos);
            if groups.is_empty() {
                self.layer_groups_by_layer_index.remove(&current);
            }
            layer_group.borrow_mut().layer_index = new_index;
            self.layer_groups_by_layer_index.entry(new_index).or_default().push(Rc::clone(layer_group));
            return;
        }
        // We're not tracking the layer, indicating that it's currently
        // disabled, so update it directly.
        layer_group.borrow_mut().layer_index = new_index;
    }

    pub(crate) fn add_layer_group(&mut self, layer_group: LayerGroupPtr) -> bool {
        let index = layer_group.borrow().layer_index;
        let groups = self.layer_groups_by_layer_index.entry(index).or_default();
        if groups.iter().any(|g| Rc::ptr_eq(g, &layer_group)) {
            false // not added
        } else {
            groups.push(layer_group);
            true // added
        }
    }

    pub(crate) fn remove_layer_group(&mut self, layer_group: &LayerGroupPtr) -> bool {
        let index = layer_group.borrow().layer_index;
        if let Some(groups) = self.layer_groups_by_layer_index.get_mut(&index)
            && let Some(pos) = groups.iter().position(|g| Rc::ptr_eq(g, layer_group))
        {
            groups.remove(pos);
            if groups.is_empty() {
                self.layer_groups_by_layer_index.remove(&index);
            }
            return true;
        }
        false
    }

    pub(crate) fn num_layer_groups(&self) -> usize {
        self.layer_groups_by_layer_index.values().map(Vec::len).sum()
    }

    /// The layer groups in multimap order (ascending layer index).
    pub(crate) fn layer_groups(&self) -> Vec<LayerGroupPtr> {
        self.layer_groups_by_layer_index.values().flatten().cloned().collect()
    }

    pub(crate) fn visit_layer_groups(&self, mut f: impl FnMut(&mut LayerGroup)) {
        for group in self.layer_groups() {
            f(&mut group.borrow_mut());
        }
    }

    pub(crate) fn visit_layer_groups_reversed(&self, mut f: impl FnMut(&mut LayerGroup)) {
        for group in self.layer_groups().into_iter().rev() {
            f(&mut group.borrow_mut());
        }
    }

    /// Updates the debug layer groups (layers: debug).
    pub(crate) fn update_debug_layer_groups(&mut self, render_tree: &RenderTree, parameters: &PaintParameters<'_>) {
        for item in render_tree.get_source_render_items() {
            item.update_debug_drawables(&mut self.debug_layer_groups, parameters);
        }

        // `layerGroup->addLayerTweaker(tweaker)`, once per group.
        let groups = &self.debug_layer_groups;
        let group_tweakers = &mut self.layer_group_tweakers;
        self.debug_layer_tweakers.retain(|type_, (group, _)| {
            let live = groups.get(type_).is_some_and(|g| Rc::ptr_eq(g, group));
            if !live {
                group_tweakers.remove(&group.borrow().id);
            }
            live
        });
        for (type_, group) in &self.debug_layer_groups {
            if self.debug_layer_tweakers.contains_key(type_) {
                continue;
            }
            let tweaker = Rc::new(RefCell::new(PolylineLayerTweaker::new(debug_line_properties(*type_))));
            let weak = Rc::downgrade(&tweaker);
            self.layer_group_tweakers.entry(group.borrow().id).or_default().push(weak);
            self.debug_layer_tweakers.insert(*type_, (Rc::clone(group), tweaker));
        }
    }

    /// The debug layer groups, in `DebugType` order (`visitDebugLayerGroups`)
    /// (layers: debug).
    pub(crate) fn debug_layer_groups(&self) -> Vec<LayerGroupPtr> {
        self.debug_layer_groups.values().cloned().collect()
    }

    /// Runs the live tweakers of the layer group and drops the expired ones.
    pub(crate) fn run_tweakers(&mut self, layer_group: &LayerGroupPtr, parameters: &PaintParameters<'_>) {
        let id = layer_group.borrow().id;
        let Some(tweakers) = self.layer_group_tweakers.get_mut(&id) else {
            return;
        };
        let mut i = 0;
        while i < tweakers.len() {
            if let Some(tweaker) = tweakers[i].upgrade() {
                tweaker.borrow_mut().execute(&mut layer_group.borrow_mut(), parameters);
                i += 1;
            } else {
                tweakers.remove(i);
            }
        }
    }

    /// Visits every render layer, in no order.
    pub(crate) fn visit_render_layers(&self, mut f: impl FnMut(&mut dyn crate::render_layer::RenderLayer)) {
        for layer in self.render_layers.values() {
            f(&mut *layer.borrow_mut());
        }
    }

    pub(crate) fn update_layers(
        &mut self,
        shaders: &ShaderRegistry,
        context: &Context,
        state: &TransformState,
        update_parameters: &Arc<UpdateParameters>,
        paint_parameters: &PaintParameters<'_>,
        render_tree: &RenderTree,
    ) {
        let items = render_tree.get_layer_render_item_map();

        let mut changes: UniqueChangeRequestVec = Vec::with_capacity(items.len() * 3);

        let mut has_3d = false;
        for item in items.values() {
            let mut render_layer = item.layer.borrow_mut();
            // C++ catches exceptions from update() and reports them through
            // onRenderError; the Rust layers don't throw.
            render_layer.update(
                shaders,
                context,
                state,
                update_parameters,
                paint_parameters,
                render_tree,
                &mut changes,
            );
            has_3d = has_3d || render_layer.needs_globe_depth();
        }
        self.add_changes(&mut changes);

        let tree_parameters = render_tree.get_parameters();
        self.sky_pass.update(
            shaders,
            context,
            state,
            paint_parameters.renderable_size,
            paint_parameters.pixel_ratio,
            tree_parameters.sky.as_ref(),
            &tree_parameters.light,
        );
        self.globe_depth_pass.update(
            shaders,
            context,
            state,
            update_parameters,
            has_3d || self.sky_pass.has_atmosphere(),
        );
    }

    pub(crate) fn get_sky_layer_group(&self) -> Option<LayerGroupPtr> {
        self.sky_pass.get_sky_layer_group().cloned()
    }

    pub(crate) fn get_atmosphere_layer_group(&self) -> Option<LayerGroupPtr> {
        self.sky_pass.get_atmosphere_layer_group().cloned()
    }

    pub(crate) fn get_globe_depth_layer_group(&self) -> Option<LayerGroupPtr> {
        self.globe_depth_pass.get_layer_group().cloned()
    }

    pub(crate) fn process_changes(&mut self) {
        let local_changes = std::mem::take(&mut self.pending_changes);
        for change in local_changes {
            match change {
                ChangeRequest::AddLayerGroup(layer_group) => {
                    self.add_layer_group(layer_group);
                }
                ChangeRequest::RemoveLayerGroup(layer_group) => {
                    self.remove_layer_group(&layer_group);
                }
                ChangeRequest::UpdateLayerGroupIndex(layer_group, new_index) => {
                    self.update_layer_index(&layer_group, new_index);
                }
                ChangeRequest::AddLayerTweaker(layer_group, tweaker) => {
                    let id = layer_group.borrow().id;
                    self.layer_group_tweakers.entry(id).or_default().push(tweaker);
                }
                // dem-heatmap
                ChangeRequest::AddRenderTarget(render_target) => {
                    self.add_render_target(render_target);
                }
                ChangeRequest::RemoveRenderTarget(render_target) => {
                    self.remove_render_target(&render_target);
                }
            }
        }
    }

    // dem-heatmap: render targets.
    pub(crate) fn add_render_target(&mut self, render_target: crate::render_target::RenderTargetPtr) -> bool {
        if self.render_targets.iter().any(|t| Rc::ptr_eq(t, &render_target)) {
            false
        } else {
            self.render_targets.push(render_target);
            true
        }
    }

    pub(crate) fn remove_render_target(&mut self, render_target: &crate::render_target::RenderTargetPtr) -> bool {
        match self.render_targets.iter().position(|t| Rc::ptr_eq(t, render_target)) {
            Some(pos) => {
                self.render_targets.remove(pos);
                true
            }
            None => false,
        }
    }

    /// The render targets, for `visitRenderTargets` (cloned, so that the
    /// visitor may use the orchestrator).
    pub(crate) fn render_targets(&self) -> Vec<crate::render_target::RenderTargetPtr> {
        self.render_targets.clone()
    }

    // query
    /// Queries the rendered features in `geometry`. `Err` if the query fails
    /// (a malformed tile).
    pub(crate) fn query_rendered_features(
        &mut self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, Error> {
        // A query answers from the layouts that arrived since the update.
        self.receive_tiles();
        let render_layers: Vec<&RenderLayerPtr> = match &options.layer_ids {
            Some(layer_ids) => layer_ids.iter().filter_map(|id| self.render_layers.get(id)).collect(),
            None => self.render_layers.values().collect(),
        };
        let borrowed: Vec<std::cell::Ref<'_, dyn crate::render_layer::RenderLayer>> =
            render_layers.iter().map(|layer| layer.borrow()).collect();
        let mut layers = crate::geometry::feature_index::QueryLayers::default();
        for layer in &borrowed {
            layers.entry(layer.get_id().to_owned()).or_insert(&**layer);
        }

        self.query_rendered_features_in(geometry, options, &layers)
    }

    fn query_rendered_symbols(
        &self,
        results_by_layer: &mut crate::geometry::feature_index::QueryResults,
        geometry: &[mln_util::geo::ScreenCoordinate],
        layers: &crate::geometry::feature_index::QueryLayers<'_>,
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<(), Error> {
        let cross_tile_symbol_index_layers: crate::geometry::feature_index::QueryLayers<'_> = layers
            .iter()
            .filter(|(_, layer)| layer.base().style.info().cross_tile_index)
            .map(|(id, layer)| (id.clone(), *layer))
            .collect();

        if cross_tile_symbol_index_layers.is_empty() {
            return Ok(());
        }
        let placement = self.placement_controller.get_placement();
        let mut rendered_symbols = placement.get_collision_index().query_rendered_symbols(&geometry.to_vec());
        let mut bucket_query_data = Vec::with_capacity(rendered_symbols.len());
        for bucket_instance_id in rendered_symbols.keys() {
            // C++ `getQueryData` throws for an unknown bucket.
            bucket_query_data.push(placement.get_query_data(*bucket_instance_id).map_err(Error::native)?);
        }
        // Although symbol query is global, symbol results are only sortable within
        // a bucket For a predictable global sort renderItems, we sort the buckets
        // based on their corresponding tile position
        bucket_query_data.sort_by(|a, b| {
            (a.tile_id.canonical.z, a.tile_id.canonical.y, a.tile_id.wrap, a.tile_id.canonical.x).cmp(&(
                b.tile_id.canonical.z,
                b.tile_id.canonical.y,
                b.tile_id.wrap,
                b.tile_id.canonical.x,
            ))
        });

        for query_data in bucket_query_data {
            let Some(feature_index) = &query_data.feature_index else {
                continue;
            };
            let symbol_features = rendered_symbols.remove(&query_data.bucket_instance_id).unwrap_or_default();
            let bucket_symbols = feature_index.lookup_symbol_features(
                &symbol_features,
                options,
                self.global_state.as_deref(),
                &cross_tile_symbol_index_layers,
                &query_data.tile_id,
                &query_data.feature_sort_order.borrow().clone(),
            )?;

            for (layer_id, features) in bucket_symbols {
                results_by_layer.entry(layer_id).or_default().extend(features);
            }
        }
        Ok(())
    }

    fn query_rendered_features_in(
        &self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        options: &crate::query::RenderedQueryOptions,
        layers: &crate::geometry::feature_index::QueryLayers<'_>,
    ) -> Result<Vec<mln_util::feature::Feature>, Error> {
        let mut source_ids: VecSet<String> = VecSet::new();
        let mut filtered_layers = crate::geometry::feature_index::QueryLayers::default();
        for (id, layer) in layers {
            if !layer.needs_rendering() || !layer.supports_zoom(self.zoom_history.last_zoom) {
                continue;
            }
            filtered_layers.insert(id.clone(), *layer);
            source_ids.insert(layer.base().style.base().source.clone());
        }

        let proj_matrix = self.transform_state.get_proj_matrix(1, false);

        let mut results_by_layer = crate::geometry::feature_index::QueryResults::default();
        for source_id in &source_ids {
            if let Some(source) = self.source_tiles(source_id) {
                let source_results = source.query_rendered_features(
                    geometry,
                    &self.transform_state,
                    &filtered_layers,
                    options,
                    self.global_state.as_deref(),
                    &proj_matrix,
                )?;
                for (layer_id, features) in source_results {
                    results_by_layer.entry(layer_id).or_insert(features);
                }
            }
        }

        self.query_rendered_symbols(&mut results_by_layer, geometry, &filtered_layers, options)?;

        let mut result = Vec::new();

        if results_by_layer.is_empty() {
            return Ok(result);
        }

        // Combine all results based on the style layer renderItems.
        // DEVIATION: C++ iterates `filteredLayers`, an unordered_map (libstdc++
        // hash order); the port goes from the topmost style layer down, the
        // order GL JS (which wrote the expectations) returns.
        let layers = self.style.as_ref().map(|style| Arc::clone(style.get_layer_list())).unwrap_or_default();
        for style_layer in layers.iter().rev() {
            let id = style_layer.get_id();
            if !filtered_layers.contains_key(id) {
                continue;
            }
            if let Some(mut features) = results_by_layer.remove(id) {
                for feature in &mut features {
                    id.clone_into(&mut feature.layer);
                }
                result.extend(features);
            }
        }

        Ok(result)
    }

    /// A copy of the placed symbols' data (the placement is shared).
    pub(crate) fn get_placed_symbols_data(&self) -> Vec<crate::text::placement::PlacedSymbolData> {
        self.placement_controller.get_placement().get_placed_symbols_data().to_vec()
    }

    /// Queries the features of the source `source_id`. `Err`: as
    /// [`query_rendered_features`](Self::query_rendered_features).
    pub(crate) fn query_source_features(
        &mut self,
        source_id: &str,
        options: &crate::query::SourceQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, Error> {
        self.receive_tiles();
        let Some(source) = self.source_tiles(source_id) else {
            return Ok(Vec::new());
        };

        source.query_source_features(options, self.global_state.as_deref())
    }

    /// Queries a feature extension of the source `source_id`; a null value
    /// for a source the style doesn't have.
    pub(crate) fn query_feature_extensions(
        &self,
        source_id: &str,
        feature: &mln_util::feature::Feature,
        extension: &str,
        extension_field: &str,
        args: Option<&feature::PropertyMap>,
    ) -> Result<crate::query::FeatureExtensionValue, Error> {
        match self.get_render_source(source_id) {
            Some(source) => source.query_feature_extensions(feature, extension, extension_field, args),
            None => Ok(crate::query::FeatureExtensionValue::default()),
        }
    }

    pub(crate) fn dump_debug_logs(&self) {
        for source in self.render_sources.values() {
            source.dump_debug_logs();
        }
        self.tile_store.dump_debug_logs();
    }

    // layers: feature state
    pub(crate) fn set_feature_state(
        &mut self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
        state: &mln_style::expression::expression::FeatureState,
    ) {
        if let Some(source) = self.source_tiles_mut(source_id) {
            source.info.feature_state.update_state(source_layer_id, feature_id, state);
        }
    }

    pub(crate) fn get_feature_state(
        &self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
    ) -> mln_style::expression::expression::FeatureState {
        match self.source_tiles(source_id) {
            Some(source) => source.info.feature_state.get_state(source_layer_id, feature_id),
            None => mln_style::expression::expression::FeatureState::default(),
        }
    }

    pub(crate) fn remove_feature_state(
        &mut self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) {
        if let Some(source) = self.source_tiles_mut(source_id) {
            source.info.feature_state.remove_state(source_layer_id, feature_id, state_key);
        }
    }

    pub(crate) fn collect_placed_symbol_data(&mut self, enable: bool) {
        self.placed_symbol_data_collected = enable;
    }

    pub(crate) fn set_tile_cache_enabled(&mut self, enable: bool) {
        self.tile_cache_enabled = enable;
        self.tile_store.set_cache_enabled(enable);
    }

    pub(crate) fn reduce_memory_use(&mut self) {
        self.filtered_layers_for_source.shrink_to_fit();
        self.tile_store.reduce_memory_use();
        self.tile_store.images().reduce_memory_use(&self.tile_store.images_used());
        self.events.emit(RendererEvent::Invalidate);
    }

    pub(crate) fn get_tile_cache_enabled(&self) -> bool {
        self.tile_cache_enabled
    }
}

impl RenderOrchestrator {
    /// Drops `value` on the renderer's workers instead of this thread. The
    /// orchestrator's drop waits for it.
    pub(crate) fn drop_on_workers<T: Send + 'static>(&self, value: T) {
        drop_on_workers(&self.thread_pool, value);
    }
}

/// Drops `value` on `thread_pool`, the orchestrator's workers, instead of
/// this thread. The orchestrator's drop waits for it.
pub(crate) fn drop_on_workers<T: Send + 'static>(thread_pool: &TaskGroup, value: T) {
    thread_pool.spawn(move || drop(value));
}

/// A value that [`RenderOrchestrator::drop_on_workers`] drops once this
/// drops.
struct DropOnWorkers<T: Send + 'static> {
    value: Option<T>,
    thread_pool: TaskGroup,
}

impl<T: Send + 'static> DropOnWorkers<T> {
    fn new(value: T, thread_pool: TaskGroup) -> Self {
        DropOnWorkers { value: Some(value), thread_pool }
    }
}

impl<T: Send + 'static> Drop for DropOnWorkers<T> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            drop_on_workers(&self.thread_pool, value);
        }
    }
}

/// Waits for its thread pool's tasks when it drops.
struct WaitOnDrop(TaskGroup);

impl Drop for WaitOnDrop {
    fn drop(&mut self) {
        self.0.wait();
    }
}

impl Drop for RenderOrchestrator {
    fn drop(&mut self) {
        self.tile_store.log_stats();
        // Destroy the render sources and their tiles before waiting for the
        // thread pool, as the C++ members' destructors do.
        self.render_sources.clear();
        self.tile_store.clear();
        self.thread_pool.wait();
    }
}
