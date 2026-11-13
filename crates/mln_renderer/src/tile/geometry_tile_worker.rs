// The in-process transport makes the worker a pool actor
// (mln_actor::actor::Actor) on the map's thread pool, as in C++. Its parent
// GeometryTile lives on the render thread and is reached through a mailbox
// (`ActorRef<GeometryTileInner>`). When the source asks for synchronous
// updates, C++ uses an `OptionalActor` that calls the worker and the tile
// directly; the worker's `parent` is then the tile itself and `SelfRef`
// is `None`.
//
// C++ wraps each message handler in `try { ... } catch (...)` and reports
// the exception to the tile (`GeometryTile::onError`). The port returns the
// exceptions as `Err` (malformed tiles, a too long geometry) and reports
// those; a panic is a bug and is not caught (`catch_unwind` can't catch it
// with `panic = "abort"` or on wasm).
//
// Port-only: the messages between the tile and its worker are data
// (`WorkerMessage`, `TileMessage`) instead of member function calls bound
// into closures, and they travel through a `TileWorkerTransport`
// (tile_worker_transport.rs), so that a worker can also run in another
// wasm instance.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mln_actor::actor::PoolActorRef;
use mln_expression::image_expression::UnavailableImages;
use mln_map_types::MapMode;
use mln_style::expression::{EvaluationContext, GlobalStateMap};
use mln_style::layer::layer_type_info;
use mln_style::layer_properties::LayerProperties;
use mln_style_types::image::{ImageDependencies, ImageMap, ImagePositions, ImageType, shared_image_map};
use mln_text::glyph::{GlyphDependencies, GlyphIDType, GlyphMap};
use mln_text::tagged_string::font_stack_hash;
use mln_tile_id::OverscaledTileID;
use mln_tile_id::geometry_tile_data::{GeometryTileData, GeometryTileFeature};
use mln_util::error::Error;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;
use serde::{Deserialize, Serialize};

use crate::bucket::BucketPtr;
use crate::bucket_parameters::BucketParameters;
use crate::geometry::feature_index::FeatureIndex;
use crate::gfx::dynamic_texture_atlas::{DynamicTextureAtlasPtr, GlyphAtlas, ImageAtlas, TextureAtlas};
use crate::group_by_layout::group_layers;
use crate::image_manager::ImageRequestPair;
use crate::layermanager;
use crate::layout::filter_groups::FilterGroups;
use crate::layout::layout::{Layout, LayoutParameters, SourceLayer};
use crate::layout::pattern_layout::PatternLayerMap;
use crate::render_layer::LayerRenderData;
use crate::text::glyph_manager::HBShapeResults;
use crate::tile::geometry_tile::LayoutResult;
use crate::tile::tile_worker_transport::TileEndpoint;
use crate::tile::worker_codec::fields;

/// The tile data the worker parses (C++ `std::unique_ptr<const
/// GeometryTileData>`, shared here with the feature index instead of
/// cloned).
pub type SharedTileData = Arc<dyn GeometryTileData + Send + Sync>;

pub use crate::gfx::dynamic_texture_atlas::ImageVersionMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Coalescing,
    NeedsParse,
    NeedsSymbolLayout,
}

/// The messages a `GeometryTile` sends its `GeometryTileWorker`: the
/// worker's public member functions. Port-only: serde writes them for a
/// worker in another wasm instance (`tile/worker_codec`).
#[derive(Serialize, Deserialize)]
pub enum WorkerMessage {
    SetData {
        #[serde(with = "fields::tile_data")]
        data: Option<SharedTileData>,
        available_images: Arc<VecSet<String>>,
        correlation_id: u64,
    },
    /// Sets the layers, with the pixel ratio to lay them out for.
    SetLayers {
        #[serde(with = "fields::layers")]
        layers: Vec<Arc<dyn LayerProperties>>,
        global_state: Option<Arc<GlobalStateMap>>,
        available_images: Arc<VecSet<String>>,
        subdivision_granularity: SubdivisionGranularitySetting,
        pixel_ratio: f32,
        correlation_id: u64,
    },
    /// Parses again with the images added since the last parse.
    SetAvailableImages {
        available_images: Arc<VecSet<String>>,
        correlation_id: u64,
    },
    Reset {
        correlation_id: u64,
    },
    SetShowCollisionBoxes {
        show_collision_boxes: bool,
        correlation_id: u64,
    },
    /// Forget the glyphs that came back missing and lay out again, which
    /// requests them again.
    RetryMissingGlyphs {
        correlation_id: u64,
    },
    GlyphsAvailable {
        glyph_map: GlyphMap,
        results: HBShapeResults,
    },
    ImagesAvailable {
        #[serde(with = "shared_image_map")]
        icon_map: ImageMap,
        #[serde(with = "shared_image_map")]
        pattern_map: ImageMap,
        version_map: ImageVersionMap,
        image_correlation_id: u64,
    },
}

impl WorkerMessage {
    /// The message's correlation id, or 0 for the glyph and image replies,
    /// which have none.
    #[must_use]
    pub fn correlation_id(&self) -> u64 {
        match self {
            WorkerMessage::SetData { correlation_id, .. }
            | WorkerMessage::SetLayers { correlation_id, .. }
            | WorkerMessage::SetAvailableImages { correlation_id, .. }
            | WorkerMessage::Reset { correlation_id }
            | WorkerMessage::SetShowCollisionBoxes { correlation_id, .. }
            | WorkerMessage::RetryMissingGlyphs { correlation_id } => *correlation_id,
            WorkerMessage::GlyphsAvailable { .. } | WorkerMessage::ImagesAvailable { .. } => 0,
        }
    }
}

/// The messages a `GeometryTileWorker` sends its `GeometryTile`: the tile's
/// member functions the worker calls. Port-only: serde writes them for a
/// worker in another wasm instance (`tile/worker_codec`).
#[derive(Serialize, Deserialize)]
pub enum TileMessage {
    Layout { result: LayoutResult, correlation_id: u64 },
    Error { error: Error, correlation_id: u64 },
    GetGlyphs(GlyphDependencies),
    GetImages(ImageRequestPair),
}

/// The worker's reference to itself: `None` for the synchronous worker, whose
/// self-sent messages are direct calls.
pub type SelfRef = Option<PoolActorRef<GeometryTileWorker>>;

/// The tile data a `GeometryTileWorker` received.
enum ReceivedData {
    /// No `set_data` since the worker was created or reset.
    Pending,
    /// The tile's data; `None` when the tile has no data.
    Received(Option<SharedTileData>),
}

pub struct GeometryTileWorker {
    self_ref: SelfRef,
    parent: Box<dyn TileEndpoint>,

    id: OverscaledTileID,
    source_id: String,
    obsolete: Arc<AtomicBool>,
    mode: MapMode,
    pixel_ratio: f32,
    capture_rendered_features: bool,

    feature_index: Option<Box<FeatureIndex>>,
    render_data: HashMap<String, LayerRenderData>,

    state: State,
    correlation_id: u64,
    image_correlation_id: u64,

    // `None` until the worker has received the layers.
    layers: Option<Vec<Arc<dyn LayerProperties>>>,
    data: ReceivedData,

    // The style's global state, used by "global-state" expressions during
    // layout.
    global_state: Option<Arc<GlobalStateMap>>,

    layouts: Vec<Box<dyn Layout>>,

    // symbols:
    pending_glyph_dependencies: GlyphDependencies,
    glyph_map: GlyphMap,
    dynamic_texture_atlas: Option<DynamicTextureAtlasPtr>,
    font_faces: Option<Arc<mln_text::glyph::FontFaces>>,

    pending_image_dependencies: ImageDependencies,
    icon_map: ImageMap,
    pattern_map: ImageMap,
    version_map: ImageVersionMap,
    available_images: Arc<VecSet<String>>,

    show_collision_boxes: bool,
    subdivision_granularity: SubdivisionGranularitySetting,
    first_load: bool,
}

impl GeometryTileWorker {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        self_ref: SelfRef,
        parent: Box<dyn TileEndpoint>,
        id: OverscaledTileID,
        source_id: String,
        obsolete: Arc<AtomicBool>,
        mode: MapMode,
        pixel_ratio: f32,
        show_collision_boxes: bool,
        dynamic_texture_atlas: Option<DynamicTextureAtlasPtr>,
        font_faces: Option<Arc<mln_text::glyph::FontFaces>>,
        capture_rendered_features: bool,
    ) -> Self {
        GeometryTileWorker {
            self_ref,
            parent,
            id,
            source_id,
            obsolete,
            mode,
            pixel_ratio,
            capture_rendered_features,
            feature_index: None,
            render_data: HashMap::new(),
            state: State::Idle,
            correlation_id: 0,
            image_correlation_id: 0,
            layers: None,
            data: ReceivedData::Pending,
            global_state: None,
            layouts: Vec::new(),
            pending_glyph_dependencies: GlyphDependencies::default(),
            glyph_map: GlyphMap::new(),
            dynamic_texture_atlas,
            font_faces,
            pending_image_dependencies: ImageDependencies::new(),
            icon_map: ImageMap::new(),
            pattern_map: ImageMap::new(),
            version_map: ImageVersionMap::new(),
            available_images: Arc::new(VecSet::new()),
            show_collision_boxes,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            first_load: true,
        }
    }

    fn obsolete(&self) -> bool {
        self.obsolete.load(Ordering::SeqCst)
    }

    /// Runs the member function a message stands for.
    pub fn handle(&mut self, message: WorkerMessage) {
        match message {
            WorkerMessage::SetData { data, available_images, correlation_id } => {
                self.set_data(data, available_images, correlation_id);
            }
            WorkerMessage::SetLayers {
                layers,
                global_state,
                available_images,
                subdivision_granularity,
                pixel_ratio,
                correlation_id,
            } => {
                self.pixel_ratio = pixel_ratio;
                self.set_layers(layers, global_state, available_images, subdivision_granularity, correlation_id);
            }
            WorkerMessage::SetAvailableImages { available_images, correlation_id } => {
                self.set_available_images(available_images, correlation_id);
            }
            WorkerMessage::Reset { correlation_id } => self.reset(correlation_id),
            WorkerMessage::SetShowCollisionBoxes { show_collision_boxes, correlation_id } => {
                self.set_show_collision_boxes(show_collision_boxes, correlation_id);
            }
            WorkerMessage::RetryMissingGlyphs { correlation_id } => self.retry_missing_glyphs(correlation_id),
            WorkerMessage::GlyphsAvailable { glyph_map, results } => self.on_glyphs_available(glyph_map, results),
            WorkerMessage::ImagesAvailable { icon_map, pattern_map, version_map, image_correlation_id } => {
                self.on_images_available(icon_map, pattern_map, version_map, image_correlation_id);
            }
        }
    }

    /// Runs `f` as the C++ `try { f } catch (...) { parent.invoke(
    /// &GeometryTile::onError, std::current_exception(), correlationID); }`.
    fn guarded(&mut self, f: impl FnOnce(&mut Self) -> Result<(), Error>) {
        let Err(error) = f(self) else {
            return;
        };
        self.parent.send(TileMessage::Error { error, correlation_id: self.correlation_id });
    }

    /*
       GeometryTileWorker is a state machine. This is its transition diagram.
       States are indicated by [state], lines are transitions triggered by
       messages, (parentheses) are actions taken on transition.

                             [Idle] <-------------------------.
                                |                             |
            set{Data,Layers}, symbolDependenciesChanged,      |
                        setShowCollisionBoxes                 |
                                |                             |
       (do parse and/or symbol layout; self-send "coalesced") |
                                v                             |
                          [Coalescing] --- coalesced ---------.
                             |   |
                 .-----------.   .---------------------.
                 |                                     |
       .--- set{Data,Layers}                setShowCollisionBoxes,
       |         |                         symbolDependenciesChanged --.
       |         |                                     |               |
       |         v                                     v               |
       .-- [NeedsParse] <-- set{Data,Layers} -- [NeedsSymbolLayout] ---.
                 |                                     |
             coalesced                             coalesced
                 |                                     |
                 v                                     v
       (do parse or symbol layout; self-send "coalesced"; goto [coalescing])

       See the C++ file for the full description.
    */

    pub fn set_data(
        &mut self,
        data: Option<SharedTileData>,
        available_images: Arc<VecSet<String>>,
        correlation_id: u64,
    ) {
        self.guarded(|this| {
            this.data = ReceivedData::Received(data);
            this.correlation_id = correlation_id;
            this.available_images = available_images;

            match this.state {
                State::Idle => {
                    this.parse()?;
                    this.coalesce();
                }
                State::Coalescing | State::NeedsParse | State::NeedsSymbolLayout => {
                    this.state = State::NeedsParse;
                }
            }
            Ok(())
        });
    }

    pub fn set_layers(
        &mut self,
        layers: Vec<Arc<dyn LayerProperties>>,
        global_state: Option<Arc<GlobalStateMap>>,
        available_images: Arc<VecSet<String>>,
        subdivision_granularity: SubdivisionGranularitySetting,
        correlation_id: u64,
    ) {
        self.guarded(|this| {
            this.layers = Some(layers);
            this.global_state = global_state;
            this.correlation_id = correlation_id;
            this.available_images = available_images;
            this.subdivision_granularity = subdivision_granularity;

            match this.state {
                State::Idle => {
                    this.parse()?;
                    this.coalesce();
                }
                State::Coalescing | State::NeedsSymbolLayout => {
                    this.state = State::NeedsParse;
                }
                State::NeedsParse => {}
            }
            Ok(())
        });
    }

    pub fn set_available_images(&mut self, available_images: Arc<VecSet<String>>, correlation_id: u64) {
        self.guarded(|this| {
            this.available_images = available_images;
            this.correlation_id = correlation_id;

            match this.state {
                State::Idle => {
                    this.parse()?;
                    this.coalesce();
                }
                State::Coalescing | State::NeedsSymbolLayout => {
                    this.state = State::NeedsParse;
                }
                State::NeedsParse => {}
            }
            Ok(())
        });
    }

    pub fn reset(&mut self, correlation_id: u64) {
        self.layers = None;
        self.data = ReceivedData::Pending;
        self.correlation_id = correlation_id;

        match self.state {
            State::Idle | State::NeedsParse => {}
            State::Coalescing | State::NeedsSymbolLayout => {
                self.state = State::NeedsParse;
            }
        }
    }

    pub fn set_show_collision_boxes(&mut self, show_collision_boxes: bool, correlation_id: u64) {
        self.guarded(|this| {
            this.show_collision_boxes = show_collision_boxes;
            this.correlation_id = correlation_id;

            match this.state {
                State::Idle => {
                    if !this.has_pending_parse_result() {
                        // Trigger parse if nothing is in flight, otherwise
                        // symbol layout will automatically pick up the change
                        this.parse()?;
                        this.coalesce();
                    }
                }
                State::Coalescing => {
                    this.state = State::NeedsSymbolLayout;
                }
                State::NeedsSymbolLayout | State::NeedsParse => {}
            }
            Ok(())
        });
    }

    /// Forgets the glyphs that came back missing and parses again, so the
    /// layout requests them again.
    pub fn retry_missing_glyphs(&mut self, correlation_id: u64) {
        self.guarded(|this| {
            for glyphs in this.glyph_map.values_mut() {
                glyphs.retain(|_, glyph| glyph.is_some());
            }
            this.correlation_id = correlation_id;

            match this.state {
                State::Idle => {
                    this.parse()?;
                    this.coalesce();
                }
                State::Coalescing | State::NeedsSymbolLayout => {
                    this.state = State::NeedsParse;
                }
                State::NeedsParse => {}
            }
            Ok(())
        });
    }

    fn symbol_dependencies_changed(&mut self) {
        self.guarded(|this| {
            match this.state {
                State::Idle => {
                    if !this.layouts.is_empty() {
                        // Layouts are created only by parsing and the parse
                        // result can only be cleared by performLayout, which
                        // also clears the layouts.
                        debug_assert!(this.has_pending_parse_result());
                        this.finalize_layout()?;
                        this.coalesce();
                    }
                }
                State::Coalescing => {
                    if !this.layouts.is_empty() {
                        this.state = State::NeedsSymbolLayout;
                    }
                }
                State::NeedsSymbolLayout | State::NeedsParse => {}
            }
            Ok(())
        });
    }

    fn coalesced(&mut self) {
        self.guarded(|this| {
            match this.state {
                State::Idle => {
                    debug_assert!(false);
                }
                State::Coalescing => {
                    this.state = State::Idle;
                }
                State::NeedsParse => {
                    this.parse()?;
                    this.coalesce();
                }
                State::NeedsSymbolLayout => {
                    // We may have entered NeedsSymbolLayout while coalescing
                    // after a performLayout. In that case, we need to start
                    // over with parsing in order to do another layout.
                    if this.has_pending_parse_result() {
                        this.finalize_layout()?;
                    } else {
                        this.parse()?;
                    }
                    this.coalesce();
                }
            }
            Ok(())
        });
    }

    /// Enters the coalescing state and sends itself `coalesced`; the
    /// synchronous worker calls it directly.
    fn coalesce(&mut self) {
        self.state = State::Coalescing;
        match &self.self_ref {
            Some(self_ref) => self_ref.invoke(|worker: &mut GeometryTileWorker| worker.coalesced()),
            // OptionalActorRef with an object: a direct call.
            None => self.coalesced(),
        }
    }

    pub fn on_glyphs_available(&mut self, new_glyph_map: GlyphMap, mut results: HBShapeResults) {
        // C++ moves the pending glyphs out of `newGlyphMap` in the first loop
        // and adds the rest (the shaped glyphs) in the second.
        let mut shaped_glyph_map = new_glyph_map.clone();
        for (font_stack, new_glyphs) in new_glyph_map {
            let glyphs = self.glyph_map.entry(font_stack).or_default();
            for (pending_font_stack, pending_glyph_ids) in &mut self.pending_glyph_dependencies.glyphs {
                // Linear lookup here to handle reverse of FontStackHash ->
                // FontStack, since dependencies need the full font stack
                // name to make a request. There should not be many
                // fontstacks to look through
                if font_stack_hash(pending_font_stack) == font_stack {
                    for (glyph_id, glyph) in &new_glyphs {
                        if pending_glyph_ids.remove(glyph_id) {
                            if glyph_id.code != 0 || glyph_id.type_ == GlyphIDType::FontPBF {
                                glyphs.entry(*glyph_id).or_insert_with(|| glyph.clone());
                            }
                            // moved-from in C++: the second loop sees an
                            // empty optional, which `emplace` won't add over
                            // this entry anyway.
                            if let Some(shaped) = shaped_glyph_map.get_mut(&font_stack) {
                                shaped.remove(glyph_id);
                            }
                        }
                    }
                }
            }
        }

        if !results.is_empty() {
            self.pending_glyph_dependencies.shapes.clear();

            for (font_stack, new_glyphs) in &shaped_glyph_map {
                let glyphs = self.glyph_map.entry(*font_stack).or_default();
                for (glyph_id, glyph) in new_glyphs {
                    if glyph_id.code != 0 || glyph_id.type_ == GlyphIDType::FontPBF {
                        glyphs.entry(*glyph_id).or_insert_with(|| glyph.clone());
                    }
                }
            }

            for layout in &mut self.layouts {
                if layout.need_finalize_symbols() {
                    layout.finalize_symbols(&mut results);
                }
            }
        }

        self.symbol_dependencies_changed();
    }

    fn request_new_dependencies(
        &mut self,
        glyph_dependencies: &GlyphDependencies,
        image_dependencies: ImageDependencies,
    ) {
        for (font_stack, glyph_ids) in &glyph_dependencies.glyphs {
            let font_glyphs = self.glyph_map.get(&font_stack_hash(font_stack));
            for glyph_id in glyph_ids {
                if font_glyphs.is_none_or(|glyphs| !glyphs.contains_key(glyph_id)) {
                    self.pending_glyph_dependencies.glyphs.entry(font_stack.clone()).or_default().insert(*glyph_id);
                }
            }
        }
        for (font_stack, type_dependencies) in &glyph_dependencies.shapes {
            for (type_, strs) in type_dependencies {
                for str_ in strs {
                    self.pending_glyph_dependencies
                        .shapes
                        .entry(font_stack.clone())
                        .or_default()
                        .entry(*type_)
                        .or_default()
                        .insert(str_.clone());
                }
            }
        }
        // Record both before requesting either, so that a glyph answer
        // handled before the image request goes out can't finalize the layout
        // with an empty icon map.
        self.pending_image_dependencies = image_dependencies;

        if !self.pending_glyph_dependencies.glyphs.is_empty() {
            self.parent.send(TileMessage::GetGlyphs(self.pending_glyph_dependencies.clone()));
        }
        if !self.pending_image_dependencies.is_empty() {
            self.image_correlation_id += 1;
            let pair = (self.pending_image_dependencies.clone(), self.image_correlation_id);
            self.parent.send(TileMessage::GetImages(pair));
        }
    }

    pub fn on_images_available(
        &mut self,
        new_icon_map: ImageMap,
        new_pattern_map: ImageMap,
        new_version_map: ImageVersionMap,
        image_correlation_id: u64,
    ) {
        if self.image_correlation_id != image_correlation_id {
            return; // Ignore outdated image request replies.
        }
        self.icon_map = new_icon_map;
        self.pattern_map = new_pattern_map;
        self.version_map = new_version_map;
        self.pending_image_dependencies.clear();
        self.symbol_dependencies_changed();
    }

    fn parse(&mut self) -> Result<(), Error> {
        // Avoid small reallocations for populated cells. If we had a total
        // feature count, this could be based on that and the cell count.
        const ESTIMATED_ELEMENTS_PER_CELL: usize = 8;

        let (ReceivedData::Received(data), Some(layers)) = (&self.data, &self.layers) else {
            return Ok(());
        };
        let data = data.clone();
        let layers = layers.clone();

        self.render_data.clear();
        self.layouts.clear();

        let mut feature_index = Box::new(FeatureIndex::new(data.clone()));

        feature_index.reserve(ESTIMATED_ELEMENTS_PER_CELL);
        self.feature_index = Some(feature_index);

        let mut glyph_dependencies = GlyphDependencies::default();
        let mut image_dependencies = ImageDependencies::new();
        let mut filter_groups = FilterGroups::default();
        let unavailable_images = UnavailableImages::start();

        // Create render layers and group by layout
        let group_map = group_layers(&layers);

        for (_, group) in &group_map {
            if self.obsolete() {
                return Ok(());
            }

            let Some(data) = &data else {
                continue; // Tile has no data.
            };

            let leader_impl = &group[0].base_impl;
            let parameters = BucketParameters {
                tile_id: self.id,
                mode: self.mode,
                pixel_ratio: self.pixel_ratio,
                layer_type: leader_impl.get_type_info(),
                global_state: self.global_state.clone(),
                retain_features_by_id: self.capture_rendered_features,
                subdivision_granularity: self.subdivision_granularity,
            };

            let Some(geometry_layer) = data.get_layer(&leader_impl.source_layer)? else {
                continue;
            };
            drop(geometry_layer);

            let layer_ids: Vec<String> = group.iter().map(|layer| layer.base_impl.id.clone()).collect();

            let feature_index = self.feature_index.as_mut().expect("feature index");
            feature_index.set_bucket_layer_ids(&leader_impl.id, &layer_ids);

            // Symbol layers and layers that support pattern properties have
            // an extra step at layout time to figure out what images/glyphs
            // are needed to render the layer. They use the intermediate
            // Layout data structure to accomplish this, and either
            // immediately create a bucket if no images/glyphs are used, or
            // the Layout is stored until the images/glyphs are available to
            // add the features to the buckets.
            if leader_impl.get_type_info().layout == layer_type_info::Layout::Required {
                let layout = layermanager::create_layout(
                    &mut LayoutParameters {
                        bucket_parameters: &parameters,
                        font_faces: self.font_faces.clone(),
                        glyph_dependencies: &mut glyph_dependencies,
                        image_dependencies: &mut image_dependencies,
                        available_images: &self.available_images,
                        filter_groups: &mut filter_groups,
                    },
                    SourceLayer::new(data.clone(), leader_impl.source_layer.clone()),
                    group,
                )?;
                let Some(mut layout) = layout else {
                    continue;
                };
                if layout.has_dependencies() {
                    self.layouts.push(layout);
                } else {
                    layout.create_bucket(
                        &ImagePositions::new(),
                        feature_index,
                        &mut self.render_data,
                        self.first_load,
                        self.show_collision_boxes,
                        &self.id.canonical,
                    )?;
                }
            } else {
                let filter = &leader_impl.filter;
                let source_layer_id = &leader_impl.source_layer;
                let Some(mut bucket) = layermanager::create_bucket(&parameters, group) else {
                    continue;
                };
                let Some(geometry_layer) = data.get_layer(source_layer_id)? else {
                    continue;
                };

                bucket.set_retain_features_by_id(self.capture_rendered_features);
                bucket.reserve_features(geometry_layer.feature_count());

                let effective_z = f32::from(self.id.overscaled_z);
                let passes = |feature: &dyn GeometryTileFeature| {
                    filter.evaluate(
                        &EvaluationContext::with_zoom_feature(effective_z, Some(feature))
                            .with_canonical_tile_id(Some(&self.id.canonical))
                            .with_global_state(self.global_state.as_deref()),
                    )
                };
                let selected = filter_groups.select(source_layer_id, &*geometry_layer, filter, passes);

                let mut i = 0;
                while !self.obsolete.load(Ordering::SeqCst) && i < geometry_layer.feature_count() {
                    if selected.as_ref().is_some_and(|selected| !selected[i]) {
                        i += 1;
                        continue;
                    }
                    let feature = geometry_layer.get_feature(i)?;
                    if selected.is_none() {
                        let passes = passes(&*feature);
                        feature.check()?;
                        if !passes {
                            i += 1;
                            continue;
                        }
                    }

                    let geometries = feature.get_geometries();
                    feature.check()?;
                    bucket.add_feature(
                        &*feature,
                        geometries,
                        &ImagePositions::new(),
                        &PatternLayerMap::new(),
                        i,
                        &self.id.canonical,
                    )?;
                    feature.check()?;
                    feature_index.insert(geometries, i, source_layer_id, &leader_impl.id);
                    i += 1;
                }

                if !bucket.has_data() {
                    continue;
                }

                let bucket: BucketPtr = Arc::from(bucket);
                for layer in group {
                    // `emplace` keeps an existing entry.
                    self.render_data
                        .entry(layer.base_impl.id.clone())
                        .or_insert_with(|| LayerRenderData { bucket: bucket.clone(), layer_properties: layer.clone() });
                }
            }
        }

        // Ask for the images that the layout's expressions passed over as
        // unavailable, so that adding one lays the tile out again.
        for id in unavailable_images.finish() {
            image_dependencies.entry(id).or_insert(ImageType::Icon);
        }
        self.request_new_dependencies(&glyph_dependencies, image_dependencies);

        self.finalize_layout()
    }

    fn has_pending_dependencies(&self) -> bool {
        for glyph_ids in self.pending_glyph_dependencies.glyphs.values() {
            if !glyph_ids.is_empty() {
                return true;
            }
        }
        !self.pending_image_dependencies.is_empty()
    }

    fn has_pending_parse_result(&self) -> bool {
        self.feature_index.is_some()
    }

    fn finalize_layout(&mut self) -> Result<(), Error> {
        if matches!(self.data, ReceivedData::Pending)
            || self.layers.is_none()
            || !self.has_pending_parse_result()
            || self.has_pending_dependencies()
        {
            return Ok(());
        }

        let mut image_atlas = ImageAtlas::default();
        let mut glyph_atlas = GlyphAtlas::default();
        if let Some(atlas) = &self.dynamic_texture_atlas {
            image_atlas = atlas.upload_icons_and_patterns(&self.icon_map, &self.pattern_map, &self.version_map);
        }
        if !self.layouts.is_empty() {
            if let Some(atlas) = &self.dynamic_texture_atlas {
                glyph_atlas = atlas.upload_glyphs(&self.glyph_map);
            }

            let mut layouts = std::mem::take(&mut self.layouts);
            for layout in &mut layouts {
                if self.obsolete() {
                    if let Some(atlas) = &self.dynamic_texture_atlas {
                        atlas.remove_textures(&glyph_atlas.texture_handles, &glyph_atlas.dynamic_texture);
                        atlas.remove_textures(&image_atlas.texture_handles, &image_atlas.dynamic_texture);
                    }
                    return Ok(());
                }

                layout.prepare_symbols(
                    &self.glyph_map,
                    &glyph_atlas.glyph_positions,
                    &self.icon_map,
                    &image_atlas.icon_positions,
                );

                if !layout.has_symbol_instances() {
                    continue;
                }

                // layout adds the bucket to buckets
                let feature_index = self.feature_index.as_mut().expect("feature index");
                layout.create_bucket(
                    &image_atlas.pattern_positions,
                    feature_index,
                    &mut self.render_data,
                    self.first_load,
                    self.show_collision_boxes,
                    &self.id.canonical,
                )?;
            }
        }

        self.layouts.clear();

        self.first_load = false;

        let layout_result = LayoutResult::new(
            std::mem::take(&mut self.render_data),
            Arc::from(self.feature_index.take().expect("feature index")),
            glyph_atlas,
            image_atlas,
            self.dynamic_texture_atlas.clone().map(|atlas| atlas as Arc<dyn TextureAtlas>),
        );
        self.parent.send(TileMessage::Layout { result: layout_result, correlation_id: self.correlation_id });
        Ok(())
    }

    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
}
