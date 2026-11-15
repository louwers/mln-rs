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

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use mln_actor::actor::PoolActorRef;
use mln_map_types::MapMode;
use mln_style::expression::GlobalStateMap;
use mln_style::layer_properties::LayerProperties;
use mln_style_types::image::{ImageDependencies, ImageMap, shared_image_map};
use mln_text::glyph::{GlyphDependencies, GlyphIDType, GlyphMap};
use mln_text::tagged_string::font_stack_hash;
use mln_tile::TileData;
use mln_tile_id::OverscaledTileID;
use mln_util::error::Error;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;
use mln_vec_map::VecSet;

pub(crate) use crate::gfx::dynamic_texture_atlas::ImageVersionMap;
use crate::gfx::dynamic_texture_atlas::{DynamicTextureAtlasPtr, GlyphAtlas, ImageAtlas, TextureAtlas};
use crate::image_manager::ImageRequestPair;
use crate::layout::tile_layout::{
    Dependencies, Discovered, LayoutInput, LayoutParams, ResolvedDependencies, build, discover,
};
use crate::text::glyph_manager::HBShapeResults;
use crate::tile::geometry_tile::LayoutResult;
use crate::tile::tile_worker_transport::TileEndpoint;
use crate::tile::worker_codec::fields;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Coalescing,
    NeedsParse,
    NeedsSymbolLayout,
}

/// The messages a `GeometryTile` sends its `GeometryTileWorker`: the
/// worker's public member functions. Port-only: the codec writes them for
/// a worker in another wasm instance (`tile/worker_codec`).
#[derive(mln_wire::Wire)]
pub enum WorkerMessage {
    SetData {
        data: Option<TileData>,
        available_images: Arc<VecSet<String>>,
        correlation_id: u64,
    },
    /// Sets the layers, with the pixel ratio to lay them out for.
    SetLayers {
        #[wire(with = "fields::layers")]
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
        #[wire(with = "shared_image_map")]
        icon_map: ImageMap,
        #[wire(with = "shared_image_map")]
        pattern_map: ImageMap,
        version_map: ImageVersionMap,
        image_correlation_id: u64,
    },
}

impl WorkerMessage {
    /// The message's correlation id, or 0 for the glyph and image replies,
    /// which have none.
    #[must_use]
    pub(crate) fn correlation_id(&self) -> u64 {
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
/// member functions the worker calls. Port-only: the codec writes them for
/// a worker in another wasm instance (`tile/worker_codec`).
#[derive(mln_wire::Wire)]
pub enum TileMessage {
    Layout { result: LayoutResult, correlation_id: u64 },
    Error { error: Error, correlation_id: u64 },
    GetGlyphs(GlyphDependencies),
    GetImages(ImageRequestPair),
}

/// The worker's reference to itself: `None` for the synchronous worker, whose
/// self-sent messages are direct calls.
pub(crate) type SelfRef = Option<PoolActorRef<GeometryTileWorker>>;

/// The tile data a `GeometryTileWorker` received.
enum ReceivedData {
    /// No `set_data` since the worker was created or reset.
    Pending,
    /// The tile's data; `None` when the tile has no data.
    Received(Option<TileData>),
}

pub(crate) struct GeometryTileWorker {
    self_ref: SelfRef,
    parent: Box<dyn TileEndpoint>,

    id: OverscaledTileID,
    obsolete: Arc<AtomicBool>,
    mode: MapMode,
    pixel_ratio: f32,
    capture_rendered_features: bool,

    /// The last parse's result, until its layout is built.
    discovered: Option<Discovered>,

    state: State,
    correlation_id: u64,
    image_correlation_id: u64,

    // `None` until the worker has received the layers.
    layers: Option<Vec<Arc<dyn LayerProperties>>>,
    data: ReceivedData,

    // The style's global state, used by "global-state" expressions during
    // layout.
    global_state: Option<Arc<GlobalStateMap>>,

    // symbols:
    pending_glyph_dependencies: GlyphDependencies,
    glyph_map: GlyphMap,
    shaping: HBShapeResults,
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
    #[expect(clippy::too_many_arguments, reason = "the worker's tile, parent, render mode and atlases, which it keeps")]
    pub(crate) fn new(
        self_ref: SelfRef,
        parent: Box<dyn TileEndpoint>,
        id: OverscaledTileID,
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
            obsolete,
            mode,
            pixel_ratio,
            capture_rendered_features,
            discovered: None,
            state: State::Idle,
            correlation_id: 0,
            image_correlation_id: 0,
            layers: None,
            data: ReceivedData::Pending,
            global_state: None,
            pending_glyph_dependencies: GlyphDependencies::default(),
            glyph_map: GlyphMap::new(),
            shaping: HBShapeResults::new(),
            dynamic_texture_atlas,
            font_faces,
            pending_image_dependencies: ImageDependencies::default(),
            icon_map: ImageMap::new(),
            pattern_map: ImageMap::new(),
            version_map: ImageVersionMap::default(),
            available_images: Arc::new(VecSet::new()),
            show_collision_boxes,
            subdivision_granularity: SubdivisionGranularitySetting::none(),
            first_load: true,
        }
    }

    /// What a layout reads: `None` until the worker has the data and the
    /// layers.
    fn layout_input(&self) -> Option<LayoutInput<'_>> {
        let (ReceivedData::Received(data), Some(layers)) = (&self.data, &self.layers) else {
            return None;
        };
        Some(LayoutInput {
            id: self.id,
            data: data.as_ref(),
            layers,
            global_state: self.global_state.as_ref(),
            font_faces: self.font_faces.as_ref(),
            available_images: &self.available_images,
            params: LayoutParams {
                mode: self.mode,
                pixel_ratio: self.pixel_ratio,
                show_collision_boxes: self.show_collision_boxes,
                subdivision_granularity: self.subdivision_granularity,
                first_load: self.first_load,
                capture_rendered_features: self.capture_rendered_features,
            },
        })
    }

    /// Runs the member function a message stands for.
    pub(crate) fn handle(&mut self, message: WorkerMessage) {
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

    pub(crate) fn set_data(
        &mut self,
        data: Option<TileData>,
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

    pub(crate) fn set_layers(
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

    pub(crate) fn set_available_images(&mut self, available_images: Arc<VecSet<String>>, correlation_id: u64) {
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

    pub(crate) fn reset(&mut self, correlation_id: u64) {
        self.layers = None;
        self.data = ReceivedData::Pending;
        self.discovered = None;
        self.correlation_id = correlation_id;

        match self.state {
            State::Idle | State::NeedsParse => {}
            State::Coalescing | State::NeedsSymbolLayout => {
                self.state = State::NeedsParse;
            }
        }
    }

    pub(crate) fn set_show_collision_boxes(&mut self, show_collision_boxes: bool, correlation_id: u64) {
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
    pub(crate) fn retry_missing_glyphs(&mut self, correlation_id: u64) {
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
                    if this.has_pending_parse_result() {
                        this.finalize_layout()?;
                        this.coalesce();
                    }
                }
                State::Coalescing => {
                    if this.has_pending_parse_result() {
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

    pub(crate) fn on_glyphs_available(&mut self, new_glyph_map: GlyphMap, results: HBShapeResults) {
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

            // A font shapes a string the same way each time, so the results
            // of earlier requests stay valid.
            for (font_stack, types) in results {
                let shaping = self.shaping.entry(font_stack).or_default();
                for (type_, strings) in types {
                    shaping.entry(type_).or_default().extend(strings);
                }
            }
        }

        self.symbol_dependencies_changed();
    }

    fn request_new_dependencies(&mut self, dependencies: &Dependencies) {
        for (font_stack, glyph_ids) in &dependencies.glyphs.glyphs {
            let font_glyphs = self.glyph_map.get(&font_stack_hash(font_stack));
            for glyph_id in glyph_ids {
                if font_glyphs.is_none_or(|glyphs| !glyphs.contains_key(glyph_id)) {
                    self.pending_glyph_dependencies.glyphs.entry(font_stack.clone()).or_default().insert(*glyph_id);
                }
            }
        }
        for (font_stack, type_dependencies) in &dependencies.glyphs.shapes {
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
        self.pending_image_dependencies.clone_from(&dependencies.images);

        if !self.pending_glyph_dependencies.glyphs.is_empty() {
            self.parent.send(TileMessage::GetGlyphs(self.pending_glyph_dependencies.clone()));
        }
        if !self.pending_image_dependencies.is_empty() {
            self.image_correlation_id += 1;
            let pair = (self.pending_image_dependencies.clone(), self.image_correlation_id);
            self.parent.send(TileMessage::GetImages(pair));
        }
    }

    pub(crate) fn on_images_available(
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
        self.discovered = None;
        let Some(input) = self.layout_input() else {
            return Ok(());
        };
        let Some(discovered) = discover(&input, &self.obsolete)? else {
            return Ok(());
        };
        self.request_new_dependencies(&discovered.dependencies);
        self.discovered = Some(discovered);

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
        self.discovered.is_some()
    }

    fn finalize_layout(&mut self) -> Result<(), Error> {
        if self.has_pending_dependencies() {
            return Ok(());
        }
        let Some(discovered) = self.discovered.take() else {
            return Ok(());
        };

        let atlas = self.dynamic_texture_atlas.as_ref();
        let image_atlas = atlas.map_or_else(ImageAtlas::default, |atlas| {
            atlas.upload_icons_and_patterns(&self.icon_map, &self.pattern_map, &self.version_map)
        });
        let glyph_atlas = match atlas {
            Some(atlas) if discovered.has_waiting_layouts() => atlas.upload_glyphs(&self.glyph_map),
            _ => GlyphAtlas::default(),
        };

        // `discovered` is only set while the worker has its data and layers.
        let Some(input) = self.layout_input() else {
            return Ok(());
        };
        let resolved = ResolvedDependencies {
            glyphs: &self.glyph_map,
            glyph_positions: &glyph_atlas.glyph_positions,
            icons: &self.icon_map,
            icon_positions: &image_atlas.icon_positions,
            pattern_positions: &image_atlas.pattern_positions,
            shaping: &self.shaping,
        };
        let Some(built) = build(discovered, &input, &resolved, &self.obsolete)? else {
            if let Some(atlas) = atlas {
                atlas.remove_textures(&glyph_atlas.texture_handles, glyph_atlas.dynamic_texture.as_ref());
                atlas.remove_textures(&image_atlas.texture_handles, image_atlas.dynamic_texture.as_ref());
            }
            return Ok(());
        };

        self.first_load = false;

        let layout_result = LayoutResult::new(
            built.render_data,
            Arc::new(built.feature_index),
            glyph_atlas,
            image_atlas,
            self.dynamic_texture_atlas.clone().map(|atlas| -> Arc<dyn TextureAtlas> { atlas }),
        );
        self.parent.send(TileMessage::Layout { result: layout_result, correlation_id: self.correlation_id });
        Ok(())
    }
}
