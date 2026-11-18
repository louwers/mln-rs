use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, LoadOp, TargetFormats};
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::shaders::layer_ubo::GlobalPaintParamsUBO;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_defines::ID_GLOBAL_PAINT_PARAMS_UBO;
use mln_gfx::{FrameImage, FrameTarget, Handoff, RendererBackend, ScissorRect, ShaderRegistry};
use mln_map_types::{MapDebugOptions, MapView};
use mln_style::layers::CustomLayerFrame;
use mln_text::GlyphCache;
use mln_text::local_glyph_rasterizer::LocalGlyphs;
use mln_tile_id::UnwrappedTileID;
use mln_util::Color;
use mln_util::event_queue::Waker;
use mln_util::identity::SimpleIdentity;
use mln_util::logging::{Event, Log};
use mln_util::pool::Pool;
use mln_util::time::Instant;

use crate::Registry;
use crate::clip_masks::ClipMasks;
use crate::frame::Frame;
use crate::frame_projection::{self, FrameProjection};
use crate::gfx::context::Context;
use crate::layer_composite::{Composite, LayerComposite};
use crate::layer_drawing::DrawOrder;
use crate::paint_parameters::{OpenPass, PaintParameters};
use crate::render_orchestrator::RenderOrchestrator;
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::renderer_event::{RenderMode, RendererEvent, RendererEvents};
use crate::tile::worker_codec::RemoteWorkers;
use crate::tile_matrix::to_projection_ubo;
use crate::update_parameters::UpdateParameters;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RenderState {
    Never,
    Partial,
    Fully,
}

/// What the last frame that [`Renderer::render`] drew spent on
/// each of its steps, on the CPU. They follow each other, and together take
/// nearly all of `render`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderTimings {
    /// The update up to symbol placement: the style's changes, the layers'
    /// values, the sources' tiles and the layers' choice of them.
    pub sources: Duration,
    /// The rest of the update: placing symbols across tiles and updating
    /// their buckets.
    pub symbols: Duration,
    /// Making the layers' draws from the tiles, with their uniforms.
    pub layers: Duration,
    /// Making the shaders when there are none, and uploading buffers and
    /// textures.
    pub upload: Duration,
    /// Encoding the render passes and submitting them.
    pub draw: Duration,
}

/// Draws a map's updates into frame targets on one backend: it keeps the
/// map's sources, tiles and layers between frames, and the GPU objects
/// they draw with.
pub struct Renderer {
    /// Dropped by hand in `tear_down`, inside the backend's hold: its custom
    /// layers' hosts free their graphics objects as the layers go, which on
    /// GL needs the context current. A safe `Drop` runs before the fields
    /// drop, so it can't wrap their drops in the hold otherwise.
    orchestrator: ManuallyDrop<RenderOrchestrator>,
    backend: Rc<dyn RendererBackend>,
    encoder: Encoder,
    /// Where the renderer reports its events, once the host gives it one.
    events: Option<RendererEvents>,
    /// Whether `tear_down` dropped the orchestrator.
    torn_down: bool,
}

/// What the renderer keeps for its frames' encodes: its GPU state, the
/// shaders, the passes' masks and composite, and what the last frame drew.
struct Encoder {
    /// What the renderer keeps over the backend's `Gpu`, which goes after
    /// the resources that `drop` drops first.
    context: Context,
    pixel_ratio: f32,
    /// Dropped in `drop` inside the hold, after the orchestrator.
    static_data: Option<RenderStaticData>,
    /// Kept across frames; dropped in `drop` inside the hold.
    clip_masks: ClipMasks,
    /// What layers drawn as a whole draw into and composite with, between
    /// frames.
    layer_composite: LayerComposite,
    render_state: RenderState,
    /// Where the last frame drew the map.
    frame_projection: Option<FrameProjection>,
    timings: RenderTimings,
}

impl Renderer {
    /// A renderer with a glyph cache of its own, which runs its background
    /// work on `pool` and draws `local_glyphs` from the device's fonts.
    ///
    /// `waker` runs, on any thread, when something the renderer waits for
    /// arrives, such as a tile's data or a font engine that got ready: the
    /// host asks the renderer's map for an update for it, as by pushing
    /// [`RendererEvent::Invalidate`] to the queue the renderer reports to
    /// ([`set_event_queue`](Self::set_event_queue)). It must not call the
    /// renderer.
    pub fn new(
        backend: Rc<dyn RendererBackend>,
        pixel_ratio: f32,
        local_glyphs: LocalGlyphs,
        waker: Waker,
        pool: &Pool,
    ) -> Self {
        Self::with_glyph_cache(backend, pixel_ratio, local_glyphs, Arc::default(), waker, pool)
    }

    /// A renderer that uses `glyph_cache`: renderers given the same
    /// `GlyphCache` share the glyphs they parse from glyph PBFs.
    pub fn with_glyph_cache(
        backend: Rc<dyn RendererBackend>,
        pixel_ratio: f32,
        local_glyphs: LocalGlyphs,
        glyph_cache: Arc<GlyphCache>,
        waker: Waker,
        pool: &Pool,
    ) -> Self {
        let engine = crate::text::font_face_engine::default_engine();
        let registry = Registry::with_builtins();
        Self::with_fonts(backend, pixel_ratio, local_glyphs, glyph_cache, engine, registry, waker, pool)
    }

    /// A renderer that uses `glyph_cache`, loads the font files of a style's
    /// font faces with `font_face_engine` instead of
    /// `font_face_engine::default_engine()`, and draws the layer types of
    /// `registry` instead of the built-in ones.
    ///
    /// The map that the renderer draws must have a registry filled by the
    /// same calls: a layer whose type `registry` lacks isn't drawn.
    #[expect(clippy::too_many_arguments, reason = "each argument is a part of the renderer that its host picks")]
    pub fn with_fonts(
        backend: Rc<dyn RendererBackend>,
        pixel_ratio: f32,
        local_glyphs: LocalGlyphs,
        glyph_cache: Arc<GlyphCache>,
        font_face_engine: Arc<dyn crate::text::font_face_engine::FontFaceEngine>,
        registry: Arc<Registry>,
        waker: Waker,
        pool: &Pool,
    ) -> Self {
        Renderer {
            orchestrator: ManuallyDrop::new(RenderOrchestrator::new(
                true,
                local_glyphs,
                glyph_cache,
                font_face_engine,
                registry,
                waker,
                pool,
            )),
            encoder: Encoder {
                context: Context::new(Rc::clone(&backend)),
                pixel_ratio,
                static_data: None,
                clip_masks: ClipMasks::default(),
                layer_composite: LayerComposite::default(),
                render_state: RenderState::Never,
                frame_projection: None,
                timings: RenderTimings::default(),
            },
            backend,
            events: None,
            torn_down: false,
        }
    }

    /// Draws at `pixel_ratio` device pixels per logical pixel from the next
    /// frame on, which should be the map's (`Map::set_pixel_ratio`). The
    /// renderer makes its shaders and draws again, and keeps its
    /// tiles. The host sizes the backend's framebuffer for the new ratio.
    pub fn set_pixel_ratio(&mut self, pixel_ratio: f32) {
        if pixel_ratio == self.encoder.pixel_ratio {
            return;
        }
        // The shaders build the pixel ratio in on some backends, so a new
        // ratio takes new shaders, and new draws that use them.
        let encoder = &mut self.encoder;
        encoder.pixel_ratio = pixel_ratio;
        encoder.static_data = None;
        encoder.clip_masks = ClipMasks::default();
        encoder.layer_composite = LayerComposite::default();
        self.orchestrator.release_shaders();
    }

    /// The device pixels per logical pixel the renderer draws at.
    #[must_use]
    pub fn pixel_ratio(&self) -> f32 {
        self.encoder.pixel_ratio
    }

    /// Runs the tiles' jobs, and the raster tiles' workers, in
    /// `workers` (by default on the renderer's pool). Set it before the
    /// first render so that every tile uses it.
    pub fn set_tile_workers(&mut self, workers: RemoteWorkers) {
        self.orchestrator.set_tile_workers(workers);
    }

    /// Takes in what arrived for the tiles since the last frame (their data,
    /// their jobs' replies, glyphs) and starts the jobs it makes ready. Each
    /// frame does this first; a host that draws frames on a clock calls it
    /// as soon as the renderer asks for an update, so that the tiles' work
    /// doesn't wait for the next frame to start.
    pub fn receive_tiles(&mut self) {
        self.orchestrator.receive_tiles();
        report(&mut self.orchestrator, self.events.as_ref());
    }

    /// Sets the queue the renderer pushes its events to: the map's, which
    /// the map's frontend passes on.
    pub fn set_event_queue(&mut self, events: Option<RendererEvents>) {
        self.events = events;
    }

    /// Draws `update_parameters` into a frame of `target`, which must be
    /// built on the renderer's backend. Returns where the frame went, or
    /// `None` when nothing was drawn: there was nothing to draw, or the
    /// target had nothing to draw into.
    ///
    /// A frame runs in two phases: the orchestrator's update brings the
    /// renderer's state to the update and returns the frame as a value, and
    /// [`encode`](Self::encode) builds the layers' GPU objects and draws
    /// from it.
    pub fn render(&mut self, update_parameters: &Arc<UpdateParameters>, target: &dyn FrameTarget) -> Option<Handoff> {
        // Held from the update on: a custom layer that the update removes or
        // gives a new host deinitializes on the backend, as on OpenGL with
        // the context current.
        let _hold = self.backend.hold();
        let orchestrator = &mut *self.orchestrator;
        let frame = orchestrator.update(update_parameters);
        report(orchestrator, self.events.as_ref());
        let frame = frame?;
        let updated = Instant::now();
        let encoder = &mut self.encoder;
        let handoff =
            encoder.encode(orchestrator, &*self.backend, self.events.as_ref(), &frame, update_parameters, target);
        if handoff.is_some() {
            encoder.timings.sources = frame.symbols_started - frame.started;
            encoder.timings.symbols = updated - frame.symbols_started;
        }
        handoff
    }

    /// Drops `value` on the renderer's workers instead of this
    /// thread, for data that would take a frame long to free, such as the
    /// sources of an earlier update. The renderer's drop waits for it.
    pub fn drop_on_workers<T: Send + 'static>(&self, value: T) {
        self.orchestrator.drop_on_workers(value);
    }

    /// What the last frame that [`render`](Self::render) drew
    /// spent on each of its steps.
    #[must_use]
    pub fn render_timings(&self) -> RenderTimings {
        self.encoder.timings
    }

    /// Where the last frame that [`render`](Self::render) drew put the map,
    /// or `None` before the first.
    #[must_use]
    pub fn frame_projection(&self) -> Option<FrameProjection> {
        self.encoder.frame_projection
    }

    /// The features drawn in `geometry`, a point or a polygon in screen
    /// coordinates. `Err` for a malformed tile.
    pub fn query_rendered_features(
        &mut self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        let features = self.orchestrator.query_rendered_features(geometry, options);
        report(&mut self.orchestrator, self.events.as_ref());
        features
    }

    pub fn query_rendered_features_at_point(
        &mut self,
        point: &mln_util::geo::ScreenCoordinate,
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        self.query_rendered_features(&[*point], options)
    }

    pub fn query_rendered_features_in_box(
        &mut self,
        min: &mln_util::geo::ScreenCoordinate,
        max: &mln_util::geo::ScreenCoordinate,
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        use mln_util::geo::ScreenCoordinate;
        self.query_rendered_features(
            &[*min, ScreenCoordinate::new(max.x, min.y), *max, ScreenCoordinate::new(min.x, max.y), *min],
            options,
        )
    }

    pub fn query_source_features(
        &mut self,
        source_id: &str,
        options: &crate::query::SourceQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        let features = self.orchestrator.query_source_features(source_id, options);
        report(&mut self.orchestrator, self.events.as_ref());
        features
    }

    /// Queries a feature extension: for a GeoJSON source with `cluster` on,
    /// the "supercluster" extension's "children", "leaves" or
    /// "expansion-zoom" of a cluster feature (one with a `cluster_id`
    /// property). Anything else is a null value. `Err` for a cluster ID the
    /// source doesn't have.
    pub fn query_feature_extensions(
        &self,
        source_id: &str,
        feature: &mln_util::feature::Feature,
        extension: &str,
        extension_field: &str,
        args: Option<&feature::PropertyMap>,
    ) -> Result<crate::query::FeatureExtensionValue, mln_util::error::Error> {
        self.orchestrator.query_feature_extensions(source_id, feature, extension, extension_field, args)
    }

    /// The data of the symbols the last placement placed, when
    /// [`collect_placed_symbol_data`](Self::collect_placed_symbol_data)
    /// turned their collection on.
    #[must_use]
    pub fn placed_symbols_data(&self) -> Vec<crate::text::placement::PlacedSymbolData> {
        self.orchestrator.placed_symbols_data()
    }

    /// Logs every render source's tiles and the image manager's state.
    pub fn dump_debug_logs(&self) {
        self.orchestrator.dump_debug_logs();
    }

    /// Merges `state` into a feature's state.
    ///
    /// An update whose feature state for the source differs from the last
    /// update's replaces the source's state with the update's, undoing what
    /// this and [`remove_feature_state`](Self::remove_feature_state) did. A
    /// renderer that a map drives takes its state from the map's
    /// `set_feature_state`.
    pub fn set_feature_state(
        &mut self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
        state: &mln_style::expression::expression::FeatureState,
    ) {
        self.orchestrator.set_feature_state(source_id, source_layer_id, feature_id, state);
    }

    /// The state of a feature.
    #[must_use]
    pub fn feature_state(
        &self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
    ) -> mln_style::expression::expression::FeatureState {
        self.orchestrator.feature_state(source_id, source_layer_id, feature_id)
    }

    /// Removes a key of a feature's state, a feature's whole state, or a
    /// source layer's. An update replaces the source's state as it does for
    /// [`set_feature_state`](Self::set_feature_state).
    pub fn remove_feature_state(
        &mut self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) {
        self.orchestrator.remove_feature_state(source_id, source_layer_id, feature_id, state_key);
    }

    pub fn collect_placed_symbol_data(&mut self, enable: bool) {
        self.orchestrator.collect_placed_symbol_data(enable);
    }

    pub fn set_tile_cache_enabled(&mut self, enable: bool) {
        self.orchestrator.set_tile_cache_enabled(enable);
    }

    /// Whether the renderer keeps tiles that left the view, to show them
    /// again without loading them.
    #[must_use]
    pub fn tile_cache_enabled(&self) -> bool {
        self.orchestrator.tile_cache_enabled()
    }

    /// Tells the hosts of the custom layers that the renderer's graphics
    /// objects are lost, so that they forget theirs, and lets go of them.
    /// Call it before the renderer goes when its device or target went first.
    pub fn lose_custom_layers(&mut self) {
        self.orchestrator.visit_render_layers(|layer| layer.lose_context());
    }

    /// Destroys what the renderer released and waits for the GPU, so that
    /// its work finishes before the app goes to the background, and lets go
    /// of what the orchestrator caches.
    pub fn reduce_memory_use(&mut self) {
        {
            let _hold = self.backend.hold();
            self.encoder.context.destroy_released();
            if let Err(error) = self.encoder.context.gpu().flush(true) {
                Log::error(Event::Render, format!("Waiting for the GPU failed: {error}"));
            }
            self.orchestrator.reduce_memory_use();
        }
        report(&mut self.orchestrator, self.events.as_ref());
    }

    /// The rendering stats, which count the renderer's resources until it
    /// releases them. A texture counts until the end of the frame that
    /// follows its release, or until [`reduce_memory_use`](Self::reduce_memory_use).
    #[must_use]
    pub fn rendering_stats(&self) -> RenderingStats {
        self.encoder.context.stats()
    }

    /// Destroys the renderer, and returns its rendering stats as it leaves
    /// them, with all its resources released.
    #[must_use]
    pub fn into_rendering_stats(mut self) -> RenderingStats {
        self.tear_down();
        self.encoder.context.stats()
    }

    pub fn clear_data(&mut self) {
        // The custom layers that go deinitialize on the backend.
        let _hold = self.backend.hold();
        self.orchestrator.clear_data();
    }
}

impl Encoder {
    /// The second phase of a frame: builds and refreshes the GPU objects of
    /// the layers' draws of `frame`, and draws them into `target`, unless
    /// the target has nothing to draw into now. It reads the style, the
    /// sources, the tiles each layer draws and the placement as the update
    /// left them, and changes only what draws them on the GPU: the atlases'
    /// textures, the tiles' buckets, which it uploads and binds to the
    /// atlases' textures, the layers' drawing and the renderer's GPU state.
    ///
    /// It uploads the atlases and the tiles' buckets; then the draw update
    /// makes the layers' draws of the frame, the draw upload uploads what
    /// they bind, and the draw section encodes the passes and submits them.
    fn encode(
        &mut self,
        orchestrator: &mut RenderOrchestrator,
        backend: &dyn RendererBackend,
        events: Option<&RendererEvents>,
        frame: &Frame,
        update_parameters: &Arc<UpdateParameters>,
        target: &dyn FrameTarget,
    ) -> Option<Handoff> {
        let start = Instant::now();
        let image = target.begin_frame()?;
        {
            let stats = self.context.rendering_stats();
            stats.num_draw_calls = 0;
            stats.num_frames += 1;
        }
        let static_data = &*self.static_data.get_or_insert_with(|| {
            let program_parameters = ProgramParameters::new(self.pixel_ratio, false);
            RenderStaticData::new(Box::new(ShaderRegistry::new(&program_parameters)))
        });

        if self.render_state == RenderState::Never {
            emit(events, RendererEvent::RenderMapStarted);
        }
        emit(events, RendererEvent::RenderFrameStarted);

        let state = &frame.transform.state;
        let renderable_size = image.size;
        let frustum_offset = state.get_frustum_offset();
        let pixel_ratio = f64::from(self.pixel_ratio);
        let scissor_rect = ScissorRect {
            x: (frustum_offset.left * pixel_ratio) as i32,
            y: (frustum_offset.top * pixel_ratio) as i32,
            width: renderable_size
                .width
                .wrapping_sub(((frustum_offset.left + frustum_offset.right) * pixel_ratio) as u32),
            height: renderable_size
                .height
                .wrapping_sub(((frustum_offset.top + frustum_offset.bottom) * pixel_ratio) as u32),
        };

        let mut parameters = PaintParameters::new(
            &mut self.context,
            &mut self.clip_masks,
            &mut self.layer_composite,
            frame,
            static_data,
            self.pixel_ratio,
            renderable_size,
            scissor_rect,
        );

        // - UPLOAD --------------------------------------------------------
        // The atlases first, whose textures the tiles bind, then the tiles'
        // buckets.
        orchestrator.upload_atlases(parameters.context);
        orchestrator.upload_sources(parameters.context);

        // - DRAW UPDATE ---------------------------------------------------
        // The layers' draws of the frame, and those of the debug overlays.
        let uploaded = Instant::now();
        orchestrator.update_layer_draws(update_parameters, &mut parameters, frame);
        let layers_updated = Instant::now();
        orchestrator.update_debug_layers(&mut parameters);

        // What the frame's update and draw update replaced goes before the
        // uploads make what replaces it.
        parameters.context.destroy_released();

        // - DRAW UPLOAD ---------------------------------------------------
        orchestrator.upload_draws(parameters.context);
        let draw_groups = DrawGroups::of(orchestrator, &parameters);
        let (proj_state, transform) = (parameters.state, parameters.transform_params);
        parameters.clip_masks.update(
            parameters.context,
            &static_data.shaders,
            proj_state.is_globe_rendering(),
            &draw_groups.clip_tiles,
            |tile| to_projection_ubo(&transform.projection_data_for_tile(tile, false)),
        );
        parameters.layer_composite.update(
            parameters.context,
            &static_data.shaders,
            image.size,
            image.targets.color,
            parameters.debug_options.contains(MapDebugOptions::OVERDRAW),
            &draw_groups.composited,
        );
        let atlas_size = orchestrator.atlases().pattern.pixel_size();
        let global_paint_params_ubo = GlobalPaintParamsUBO {
            pattern_atlas_texsize: [atlas_size.width as f32, atlas_size.height as f32],
            units_to_pixels: [1.0 / parameters.pixels_to_gl_units[0], 1.0 / parameters.pixels_to_gl_units[1]],
            world_size: [renderable_size.width as f32, renderable_size.height as f32],
            camera_to_center_distance: state.get_camera_to_center_distance(),
            symbol_fade_change: parameters.symbol_fade_change,
            aspect_ratio: state.get_size().aspect_ratio(),
            pixel_ratio: parameters.pixel_ratio,
            map_zoom: state.get_zoom() as f32,
            pad1: 0.0,
        };
        parameters.context.write_global_uniform_buffer(ID_GLOBAL_PAINT_PARAMS_UBO, &global_paint_params_ubo);
        let layers_uploaded = Instant::now();

        // - DRAW ----------------------------------------------------------
        // The passes hold the backend's `Gpu` until the frame is submitted.
        // The 3D layers draw into depth from 0 to the passes' depth range
        // size, which the draw groups' count sets, and custom layers record
        // their work with the projection that the frame's result reports.
        let depth_range_size = 1.0
            - ((draw_groups.count + 2) * PaintParameters::NUM_SUBLAYERS as usize) as f32
                * PaintParameters::DEPTH_EPSILON;
        let frame_projection =
            frame_projection::compute(&frame.transform, depth_range_size, parameters.context.caps().clip_depth);
        parameters.custom_layer_frame = Some(CustomLayerFrame {
            view: MapView::from_state(state, pixel_ratio),
            projection: frame_projection,
            width: renderable_size.width,
            height: renderable_size.height,
        });
        let mut gpu = backend.gpu();
        draw(orchestrator, &mut *gpu, &mut parameters, frame, &image, draw_groups.count, depth_range_size);
        let start_rendering = Instant::now();
        if let Err(error) = parameters.end_pass(&mut *gpu).and_then(|()| gpu.flush(false)) {
            Log::error(Event::Render, format!("Submitting the frame failed: {error}"));
        }
        drop(gpu);
        let handoff = target.end_frame();
        let submitted = Instant::now();

        // - FINISH --------------------------------------------------------
        let context = &mut *parameters.context;
        {
            let stats = context.rendering_stats();
            // The frame read back counts as a framebuffer, as an offscreen
            // texture does.
            stats.num_frame_buffers = i32::from(handoff == Handoff::ReadBack);
            stats.rendering_time = (submitted - start_rendering).as_secs_f64();
        }
        // What the uploads and the passes replaced.
        context.destroy_released();
        {
            let stats = context.rendering_stats();
            stats.encoding_time = frame.started.elapsed().as_secs_f64() - stats.rendering_time;
        }
        let stats = Arc::new(context.stats());
        drop(parameters);

        self.timings.layers = layers_updated - uploaded;
        self.timings.upload = (uploaded - start) + (layers_uploaded - layers_updated);
        self.timings.draw = submitted - layers_uploaded;

        emit(
            events,
            RendererEvent::RenderFrameFinished {
                mode: if frame.loaded { RenderMode::Full } else { RenderMode::Partial },
                needs_repaint: frame.needs_repaint,
                placement_changed: frame.placement_changed,
                still_image_request_id: if update_parameters.still_image_request {
                    update_parameters.still_image_request_id
                } else {
                    0
                },
                stats,
            },
        );

        if !frame.loaded {
            self.render_state = RenderState::Partial;
        } else if self.render_state != RenderState::Fully {
            self.render_state = RenderState::Fully;
            emit(events, RendererEvent::RenderMapFinished);
        }

        self.frame_projection = Some(frame_projection);
        Some(handoff)
    }
}

/// What the frame's draw groups need of the passes: their number, which sets
/// the passes' depth range; the masks of the tiles they clip to and, when
/// any of them uses the stencil buffer, of the tile that clears it; and the
/// groups drawn as a whole, each into the layer composite's texture and then
/// onto the frame.
struct DrawGroups {
    count: usize,
    clip_tiles: Vec<UnwrappedTileID>,
    composited: Vec<(SimpleIdentity, Composite)>,
}

impl DrawGroups {
    /// The draw groups of `orchestrator`'s layers. Draw groups of one source
    /// clip to the same tiles, which are added once.
    fn of(orchestrator: &RenderOrchestrator, parameters: &PaintParameters<'_>) -> DrawGroups {
        let mut groups = DrawGroups { count: 0, clip_tiles: Vec::new(), composited: Vec::new() };
        let mut last: Option<&[UnwrappedTileID]> = None;
        orchestrator.visit_draw_groups(|group| {
            groups.count += 1;
            if let Some(tiles) = group.stencil_tiles()
                && last.is_none_or(|last| last != tiles)
            {
                groups.clip_tiles.extend(tiles.iter().copied());
                last = Some(tiles);
            }
            groups.composited.extend(group.composite());
        });
        if last.is_some() {
            groups.clip_tiles.extend(parameters.stencil_clear_tile());
        }
        if !groups.clip_tiles.is_sorted() {
            groups.clip_tiles.sort_unstable();
        }
        groups.clip_tiles.dedup();
        groups
    }
}

/// The draw section of a frame's encode: encodes the passes of `frame` into
/// `image` through `gpu`, which holds the frame's work until it is
/// submitted, each pass drawing the `num_draw_groups` draw groups at depths
/// below `depth_range_size`.
fn draw(
    orchestrator: &mut RenderOrchestrator,
    gpu: &mut dyn Gpu,
    parameters: &mut PaintParameters<'_>,
    frame: &Frame,
    image: &FrameImage,
    num_draw_groups: usize,
    depth_range_size: f32,
) {
    // Custom layers record their own work before the first pass.
    for &index in frame.layers.keys() {
        orchestrator.layer_mut(index as usize).prerender(gpu, parameters);
    }

    // The 3D pass: the 3D layers draw bottom to top. No pass is open, so
    // the draw groups draw nothing but into their own targets.
    if frame.has_3d {
        parameters.pass = RenderPass::Pass3D;
        parameters.depth_range_size = depth_range_size;
        parameters.current_layer = (num_draw_groups as u32).wrapping_sub(1);
        orchestrator.render_draw_groups(gpu, parameters, DrawOrder::BottomUp);
    }

    // The layers' render targets, which the passes sample.
    orchestrator.visit_render_layers(|layer| layer.render_targets(gpu, parameters));

    // The frame's pass, cleared to the backdrop, which shows where there are
    // no tiles.
    let color = if parameters.debug_options.contains(MapDebugOptions::OVERDRAW) {
        Color::black()
    } else {
        frame.background_color
    };
    let pass = OpenPass {
        label: "main buffer",
        color: Some(ColorAttachment {
            texture: parameters.layer_composite.frame_texture().unwrap_or(image.color),
            load: LoadOp::Clear(color),
        }),
        depth_stencil: image.depth_stencil.map(|texture| DepthStencilAttachment {
            texture,
            depth: LoadOp::Clear(1.0),
            stencil: LoadOp::Clear(0),
        }),
        size: image.size,
        targets: image.targets,
    };
    if let Err(error) = parameters.begin_pass(gpu, pass) {
        Log::error(Event::Render, format!("Beginning the frame's pass failed: {error}"));
    }

    // The sky.
    if let Some(sky) = orchestrator.sky() {
        parameters.pass = RenderPass::Translucent;
        parameters.depth_range_size = depth_range_size;
        parameters.current_layer = num_draw_groups as u32 + 1;
        sky.render(gpu, parameters);
    }

    // The opaque pass: front to back with the depth buffer, except on the
    // globe, where 2D layers carry no depth and are painted bottom to top.
    parameters.pass = RenderPass::Opaque;
    parameters.depth_range_size = depth_range_size;
    if frame.transform.state.is_globe_rendering() {
        parameters.current_layer = (num_draw_groups as u32).wrapping_sub(1);
        orchestrator.render_draw_groups(gpu, parameters, DrawOrder::BottomUp);
    } else {
        parameters.current_layer = 0;
        orchestrator.render_draw_groups(gpu, parameters, DrawOrder::TopDown);
    }

    // The translucent pass. The planet goes into the depth buffer first, so
    // 3D geometry behind the horizon stays hidden.
    parameters.pass = RenderPass::Translucent;
    parameters.depth_range_size = depth_range_size;
    orchestrator.globe_depth().render(gpu, parameters);
    parameters.current_layer = (num_draw_groups as u32).wrapping_sub(1);
    orchestrator.render_draw_groups(gpu, parameters, DrawOrder::BottomUp);

    // The atmosphere.
    if let Some(atmosphere) = orchestrator.atmosphere() {
        parameters.pass = RenderPass::Translucent;
        parameters.depth_range_size = depth_range_size;
        parameters.current_layer = num_draw_groups as u32 + 1;
        atmosphere.render(gpu, parameters);
    }

    // The debug overlays.
    parameters.current_layer = 0;
    for debug in orchestrator.debug_layers() {
        debug.render(gpu, parameters);
        parameters.current_layer += 1;
    }

    // The frame, where it drew into the layer composite's texture, copied
    // into the target.
    parameters.present_frame(
        gpu,
        OpenPass {
            label: "main buffer",
            color: Some(ColorAttachment { texture: image.color, load: LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 0.0)) }),
            depth_stencil: None,
            size: image.size,
            targets: TargetFormats { color: image.targets.color, depth_stencil: None },
        },
    );
}

/// Pushes what `orchestrator` reported since the last call to `events`, the
/// renderer's event queue, or drops it when there is no queue.
fn report(orchestrator: &mut RenderOrchestrator, events: Option<&RendererEvents>) {
    for event in orchestrator.take_events() {
        emit(events, event);
    }
}

/// Pushes `event` to `events`, the renderer's event queue, if it has one.
fn emit(events: Option<&RendererEvents>, event: RendererEvent) {
    if let Some(events) = events {
        events.push(event);
    }
}

impl Renderer {
    /// Drops the renderer's resources inside the backend's hold, and
    /// destroys what the context made and what the renderer released. Once:
    /// the renderer is then only dropped.
    fn tear_down(&mut self) {
        if self.torn_down {
            return;
        }
        self.torn_down = true;
        let _hold = self.backend.hold();
        // SAFETY: `torn_down` drops `orchestrator` once, here, and nothing
        // reads it afterwards: the renderer only drops after this, and the
        // remaining fields' drops don't touch it.
        #[expect(unsafe_code, reason = "drops the orchestrator inside the backend's hold; see its field")]
        unsafe {
            ManuallyDrop::drop(&mut self.orchestrator);
        }
        let encoder = &mut self.encoder;
        encoder.clip_masks = ClipMasks::default();
        encoder.layer_composite = LayerComposite::default();
        encoder.static_data = None;
        encoder.context.release_all();
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.tear_down();
    }
}
