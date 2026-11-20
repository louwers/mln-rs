use std::sync::Arc;
use std::time::Duration;

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, LoadOp, TargetFormats};
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::shaders::layer_ubo::GlobalPaintParamsUBO;
use mln_gfx::shaders::shader_defines::ID_GLOBAL_PAINT_PARAMS_UBO;
use mln_gfx::{FrameImage, Handoff, ScissorRect};
use mln_map_types::MapDebugOptions;
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
use crate::render_layer::end_hosts;
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
/// each of its steps, on the CPU, from its [update](Renderer::update) on.
/// They follow each other, and together take nearly all of `update` and
/// `render`.
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

/// Draws a map's updates into the frames of one backend's [`Gpu`]: it keeps
/// the map's sources, tiles and layers between frames, and the GPU objects
/// they draw with.
///
/// The renderer holds no `Gpu`. Each call that makes, draws or frees its GPU
/// objects takes the one it was made for, which its host lends it, as it
/// does to the target that the renderer draws into. A frame runs in the
/// host, with one `Gpu` lent to all of it:
///
/// ```ignore
/// let frame = renderer.update(gpu, &update)?;
/// let image = target.begin_frame(gpu)?;
/// let drawn = renderer.render(gpu, &image, frame);
/// let handoff = target.end_frame(gpu);
/// renderer.finish_frame(gpu, drawn, handoff);
/// ```
///
/// [`tear_down`](Self::tear_down) frees the renderer's GPU objects. A
/// renderer dropped without it leaves them to the `Gpu`, which frees them
/// when it drops, and its custom layers' hosts hear that their graphics
/// objects are lost.
pub struct Renderer {
    orchestrator: RenderOrchestrator,
    encoder: Encoder,
    /// Where the renderer reports its events, once the host gives it one.
    events: Option<RendererEvents>,
}

/// A frame that [`Renderer::update`] brought the renderer's state to, for
/// [`Renderer::render`] to draw.
#[must_use = "an updated frame is drawn"]
pub struct UpdatedFrame {
    frame: Frame,
    parameters: Arc<UpdateParameters>,
    updated: Instant,
}

/// A frame that [`Renderer::render`] drew, for
/// [`Renderer::finish_frame`] once the target handed it off.
#[must_use = "a drawn frame is finished once its target handed it off"]
pub struct DrawnFrame {
    mode: RenderMode,
    needs_repaint: bool,
    /// When the frame's placement wants the next frame, if it does.
    repaint_at: Option<Instant>,
    placement_changed: bool,
    /// The `still_image_request_id` of a still image's frame, or 0.
    still_image_request_id: u64,
    started: Instant,
    symbols_started: Instant,
    updated: Instant,
    layers_uploaded: Instant,
    start_rendering: Instant,
}

/// What the renderer keeps for its frames' encodes: its GPU state, the
/// shaders, the passes' masks and composite, and what the last frame drew.
struct Encoder {
    /// What the renderer keeps over the backend's `Gpu`, which the
    /// tear-down destroys after what the other fields hold.
    context: Context,
    pixel_ratio: f32,
    /// Reset in the tear-down, after the orchestrator.
    static_data: RenderStaticData,
    /// Kept across frames; dropped in the tear-down.
    clip_masks: ClipMasks,
    /// What layers drawn as a whole draw into and composite with, between
    /// frames.
    layer_composite: LayerComposite,
    /// Whether the context's shaders and pipelines are of an earlier pixel
    /// ratio or overdraw setting, which the next update's render layers no
    /// longer draw with.
    shaders_replaced: bool,
    render_state: RenderState,
    /// Where the last frame drew the map.
    frame_projection: Option<FrameProjection>,
    timings: RenderTimings,
}

impl Renderer {
    /// A renderer with a glyph cache of its own, which draws the layer types
    /// of `registry`, runs its background work on `pool` and draws
    /// `local_glyphs` from the device's fonts.
    ///
    /// The map that the renderer draws must have a registry filled by the
    /// same calls: a layer whose type `registry` lacks isn't drawn.
    ///
    /// `waker` runs, on any thread, when something the renderer waits for
    /// arrives, such as a tile's data or a font engine that got ready: the
    /// host asks the renderer's map for an update for it, as by pushing
    /// [`RendererEvent::Invalidate`] to the queue the renderer reports to
    /// ([`set_event_queue`](Self::set_event_queue)). It must not call the
    /// renderer.
    ///
    /// The renderer draws with `gpu` and only with it: every later call
    /// that takes a `Gpu` takes this one.
    pub fn new(
        gpu: &dyn Gpu,
        pixel_ratio: f32,
        local_glyphs: LocalGlyphs,
        registry: Arc<Registry>,
        waker: Waker,
        pool: &Pool,
    ) -> Self {
        Self::with_glyph_cache(gpu, pixel_ratio, local_glyphs, Arc::default(), registry, waker, pool)
    }

    /// A renderer that uses `glyph_cache`: renderers given the same
    /// `GlyphCache` share the glyphs they parse from glyph PBFs.
    pub fn with_glyph_cache(
        gpu: &dyn Gpu,
        pixel_ratio: f32,
        local_glyphs: LocalGlyphs,
        glyph_cache: Arc<GlyphCache>,
        registry: Arc<Registry>,
        waker: Waker,
        pool: &Pool,
    ) -> Self {
        let engine = crate::text::font_face_engine::default_engine();
        Self::with_fonts(gpu, pixel_ratio, local_glyphs, glyph_cache, engine, registry, waker, pool)
    }

    /// A renderer that uses `glyph_cache`, loads the font files of a style's
    /// font faces with `font_face_engine` instead of
    /// `font_face_engine::default_engine()`.
    #[expect(clippy::too_many_arguments, reason = "each argument is a part of the renderer that its host picks")]
    pub fn with_fonts(
        gpu: &dyn Gpu,
        pixel_ratio: f32,
        local_glyphs: LocalGlyphs,
        glyph_cache: Arc<GlyphCache>,
        font_face_engine: Arc<dyn crate::text::font_face_engine::FontFaceEngine>,
        registry: Arc<Registry>,
        waker: Waker,
        pool: &Pool,
    ) -> Self {
        Renderer {
            orchestrator: RenderOrchestrator::new(
                true,
                local_glyphs,
                glyph_cache,
                font_face_engine,
                registry,
                waker,
                pool,
            ),
            encoder: Encoder {
                context: Context::new(*gpu.caps(), pixel_ratio),
                pixel_ratio,
                static_data: RenderStaticData::default(),
                clip_masks: ClipMasks::default(),
                layer_composite: LayerComposite::default(),
                shaders_replaced: false,
                render_state: RenderState::Never,
                frame_projection: None,
                timings: RenderTimings::default(),
            },
            events: None,
        }
    }

    /// Draws at `pixel_ratio` device pixels per logical pixel from the next
    /// frame on, which should be the map's (`Map::set_pixel_ratio`). The
    /// renderer makes its shaders and draws again, frees the ones they
    /// replace, and keeps its tiles. The host sizes the backend's
    /// framebuffer for the new ratio.
    pub fn set_pixel_ratio(&mut self, pixel_ratio: f32) {
        if pixel_ratio == self.encoder.pixel_ratio {
            return;
        }
        // The shaders build the pixel ratio in on some backends, so a new
        // ratio takes new shaders, and new draws that use them.
        self.replace_shaders(pixel_ratio, self.encoder.context.overdraw());
    }

    /// Compiles the shaders at `pixel_ratio`, with the overdraw inspector if
    /// `overdraw`, from the next update on. The render layers' draws go with
    /// that update, which makes the layers again, and the old shaders after
    /// it.
    fn replace_shaders(&mut self, pixel_ratio: f32, overdraw: bool) {
        let encoder = &mut self.encoder;
        encoder.pixel_ratio = pixel_ratio;
        encoder.context.set_program_parameters(pixel_ratio, overdraw);
        encoder.clip_masks = ClipMasks::default();
        encoder.layer_composite = LayerComposite::default();
        encoder.shaders_replaced = true;
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

    /// Brings the renderer's state to `update_parameters`: its style,
    /// sources, tiles, layers and placement. Returns the frame to draw,
    /// which the host [renders](Self::render) into a frame of its target,
    /// or `None` when there is nothing to draw: a still image that hasn't
    /// loaded yet. The hosts that layers the update removed or restyled let
    /// go of end on `gpu`.
    pub fn update(&mut self, gpu: &mut dyn Gpu, update_parameters: &Arc<UpdateParameters>) -> Option<UpdatedFrame> {
        // The overdraw inspector takes shaders and color modes of its own.
        let overdraw = update_parameters.debug_options.contains(MapDebugOptions::OVERDRAW);
        if overdraw != self.encoder.context.overdraw() {
            self.replace_shaders(self.encoder.pixel_ratio, overdraw);
        }
        let orchestrator = &mut self.orchestrator;
        let frame = orchestrator.update(update_parameters);
        report(orchestrator, self.events.as_ref());
        let encoder = &mut self.encoder;
        end_hosts(gpu, orchestrator.take_ended_hosts());
        if std::mem::take(&mut encoder.shaders_replaced) {
            encoder.context.release_shaders();
        }
        let frame = frame?;
        Some(UpdatedFrame { frame, parameters: Arc::clone(update_parameters), updated: Instant::now() })
    }

    /// Draws `frame`, which the last [`update`](Self::update) returned, into
    /// `image`, textures of `gpu` that the host's target began a frame with,
    /// and submits the frame's passes. Returns the frame, which the host
    /// ends on its target and then [finishes](Self::finish_frame).
    pub fn render(&mut self, gpu: &mut dyn Gpu, image: &FrameImage, frame: UpdatedFrame) -> DrawnFrame {
        self.encoder.encode(gpu, &mut self.orchestrator, self.events.as_ref(), image, frame)
    }

    /// Ends `frame`, which [`render`](Self::render) drew, once the host's
    /// target handed it off as `handoff`: destroys what the frame replaced
    /// and reports the frame with its rendering stats.
    #[expect(clippy::needless_pass_by_value, reason = "a frame is finished once: the call takes it")]
    pub fn finish_frame(&mut self, gpu: &mut dyn Gpu, frame: DrawnFrame, handoff: Handoff) {
        self.encoder.finish(gpu, self.events.as_ref(), &frame, handoff);
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

    /// Tells the hosts that layers draw through
    /// ([`LayerHost`](crate::layer::LayerHost)) that the renderer's graphics
    /// objects are lost, so that they forget theirs, and lets go of them.
    /// Call it before the renderer goes when its device or target went first.
    pub fn lose_layer_hosts(&mut self) {
        self.orchestrator.visit_render_layers(|layer| layer.lose_context());
    }

    /// Lets go of what the orchestrator caches, destroys what it and the
    /// renderer released, and waits for `gpu`, so that its work finishes
    /// before the app goes to the background.
    pub fn reduce_memory_use(&mut self, gpu: &mut dyn Gpu) {
        self.orchestrator.reduce_memory_use();
        self.encoder.context.destroy_released(gpu);
        if let Err(error) = gpu.flush(true) {
            Log::error(Event::Render, format!("Waiting for the GPU failed: {error}"));
        }
        report(&mut self.orchestrator, self.events.as_ref());
    }

    /// The rendering stats, which count the renderer's resources until it
    /// destroys them: a buffer or texture counts until the end of the frame
    /// that follows its release, or until
    /// [`reduce_memory_use`](Self::reduce_memory_use).
    #[must_use]
    pub fn rendering_stats(&self) -> RenderingStats {
        self.encoder.context.stats()
    }

    /// Lets go of the map's data: its sources, tiles and layers, which the
    /// next update loads again. The custom layers that go hear through
    /// `gpu` that the renderer stopped.
    pub fn clear_data(&mut self, gpu: &mut dyn Gpu) {
        self.orchestrator.clear_data();
        end_hosts(gpu, self.orchestrator.take_ended_hosts());
    }

    /// Destroys the renderer's GPU objects on `gpu`, and tells its custom
    /// layers' hosts that it stopped. Returns its rendering stats as it
    /// leaves them, with all its resources released.
    pub fn tear_down(self, gpu: &mut dyn Gpu) -> RenderingStats {
        let Renderer { mut orchestrator, mut encoder, .. } = self;
        // The layers' GPU objects go to the context's queue, and their
        // custom layers' hosts to the ended ones.
        orchestrator.leave_layers();
        let ended = orchestrator.take_ended_hosts();
        drop(orchestrator);
        encoder.clip_masks = ClipMasks::default();
        encoder.layer_composite = LayerComposite::default();
        encoder.static_data = RenderStaticData::default();
        encoder.context.tear_down(gpu);
        end_hosts(gpu, ended);
        encoder.context.stats()
    }
}

impl Encoder {
    /// The second phase of a frame: builds and refreshes the GPU objects of
    /// the layers' draws of `frame` on `gpu`, draws them into `image` and
    /// submits them. It reads the style, the sources, the tiles each layer
    /// draws and the placement as the update left them, and changes only
    /// what draws them on the GPU: the atlases' textures, the tiles'
    /// buckets, which it uploads and binds to the atlases' textures, the
    /// layers' drawing and the renderer's GPU state.
    ///
    /// It uploads the atlases and the tiles' buckets; then the draw update
    /// makes the layers' draws of the frame, the draw upload uploads what
    /// they bind, and the draw section encodes the passes and submits them.
    fn encode(
        &mut self,
        gpu: &mut dyn Gpu,
        orchestrator: &mut RenderOrchestrator,
        events: Option<&RendererEvents>,
        image: &FrameImage,
        frame: UpdatedFrame,
    ) -> DrawnFrame {
        let UpdatedFrame { frame, parameters: update_parameters, updated } = frame;
        let start = Instant::now();
        {
            let stats = self.context.rendering_stats();
            stats.num_draw_calls = 0;
            stats.num_frames += 1;
        }
        let static_data = &self.static_data;

        if self.render_state == RenderState::Never {
            emit(events, RendererEvent::RenderMapStarted);
        }
        emit(events, RendererEvent::RenderFrameStarted);

        let state = &frame.transform.view;
        let renderable_size = image.size;
        let frustum_offset = state.frustum_offset();
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

        let composites_with_frame_depth =
            image.depth_stencil.is_some_and(|texture| gpu.attaches_with_any_color(texture));
        let mut parameters = PaintParameters::new(
            &mut self.context,
            &mut self.clip_masks,
            &mut self.layer_composite,
            composites_with_frame_depth,
            &frame,
            static_data,
            self.pixel_ratio,
            renderable_size,
            scissor_rect,
        );

        // - UPLOAD --------------------------------------------------------
        // The atlases first, whose textures the tiles bind, then the tiles'
        // buckets.
        orchestrator.upload_atlases(gpu, parameters.context);
        orchestrator.upload_sources(parameters.context);

        // - DRAW UPDATE ---------------------------------------------------
        // The layers' draws of the frame, and those of the debug overlays.
        let uploaded = Instant::now();
        orchestrator.update_layer_draws(gpu, &update_parameters, &mut parameters, &frame);
        let layers_updated = Instant::now();
        orchestrator.update_debug_layers(gpu, &mut parameters);

        // What the frame's update and draw update replaced goes before the
        // uploads make what replaces it.
        parameters.context.destroy_released(gpu);

        // - DRAW UPLOAD ---------------------------------------------------
        orchestrator.upload_draws(gpu, parameters.context);
        let draw_groups = DrawGroups::of(orchestrator, &parameters);
        let (proj_state, transform) = (parameters.view, parameters.transform_params);
        parameters.clip_masks.update(gpu, parameters.context, proj_state.is_globe(), &draw_groups.clip_tiles, |tile| {
            to_projection_ubo(&transform.projection_data_for_tile(tile, false))
        });
        parameters.layer_composite.update(
            gpu,
            parameters.context,
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
            camera_to_center_distance: state.camera_to_center_distance() as f32,
            symbol_fade_change: parameters.symbol_fade_change,
            aspect_ratio: state.size().aspect_ratio(),
            pixel_ratio: parameters.pixel_ratio,
            map_zoom: state.zoom() as f32,
            pad1: 0.0,
        };
        parameters.context.write_global_uniform_buffer(gpu, ID_GLOBAL_PAINT_PARAMS_UBO, &global_paint_params_ubo);
        // What the uploads replaced goes before the passes make what
        // replaces it, such as a draw's geometry.
        parameters.context.destroy_released(gpu);
        let layers_uploaded = Instant::now();

        // - DRAW ----------------------------------------------------------
        // The 3D layers draw into depth from 0 to the passes' depth range
        // size, which the draw groups' count sets, and layers that draw
        // through a host hand it the projection that the frame's result
        // reports.
        let depth_range_size = 1.0
            - ((draw_groups.count + 2) * PaintParameters::NUM_SUBLAYERS as usize) as f32
                * PaintParameters::DEPTH_EPSILON;
        let frame_projection =
            frame_projection::compute(&frame.transform, depth_range_size, parameters.context.caps().clip_depth);
        parameters.frame_projection = Some(frame_projection);
        draw(orchestrator, gpu, &mut parameters, &frame, image, draw_groups.count, depth_range_size);
        let start_rendering = Instant::now();
        if let Err(error) = parameters.end_pass(gpu).and_then(|()| gpu.flush(false)) {
            Log::error(Event::Render, format!("Submitting the frame failed: {error}"));
        }
        drop(parameters);

        self.timings.layers = layers_updated - uploaded;
        self.timings.upload = (uploaded - start) + (layers_uploaded - layers_updated);
        self.frame_projection = Some(frame_projection);
        DrawnFrame {
            mode: if frame.loaded { RenderMode::Full } else { RenderMode::Partial },
            needs_repaint: frame.needs_repaint,
            repaint_at: frame.repaint_at,
            placement_changed: frame.placement_changed,
            still_image_request_id: if update_parameters.still_image_request {
                update_parameters.still_image_request_id
            } else {
                0
            },
            started: frame.started,
            symbols_started: frame.symbols_started,
            updated,
            layers_uploaded,
            start_rendering,
        }
    }

    /// The end of a frame that [`encode`](Self::encode) drew, once its
    /// target handed it off as `handoff`: destroys what the uploads and the
    /// passes replaced, and reports the frame with the rendering stats.
    fn finish(&mut self, gpu: &mut dyn Gpu, events: Option<&RendererEvents>, frame: &DrawnFrame, handoff: Handoff) {
        let DrawnFrame {
            mode,
            needs_repaint,
            repaint_at,
            placement_changed,
            still_image_request_id,
            started,
            symbols_started,
            updated,
            layers_uploaded,
            start_rendering,
        } = *frame;
        let submitted = Instant::now();
        let context = &mut self.context;
        {
            let stats = context.rendering_stats();
            // The frame read back counts as a framebuffer, as an offscreen
            // texture does.
            stats.num_frame_buffers = i32::from(handoff == Handoff::ReadBack);
            stats.rendering_time = (submitted - start_rendering).as_secs_f64();
        }
        // What the uploads and the passes replaced.
        context.destroy_released(gpu);
        {
            let stats = context.rendering_stats();
            stats.encoding_time = started.elapsed().as_secs_f64() - stats.rendering_time;
        }
        let stats = Arc::new(context.stats());

        self.timings.sources = symbols_started - started;
        self.timings.symbols = updated - symbols_started;
        self.timings.draw = submitted - layers_uploaded;

        emit(
            events,
            RendererEvent::RenderFrameFinished {
                mode,
                needs_repaint,
                repaint_at,
                placement_changed,
                still_image_request_id,
                stats,
            },
        );

        if mode == RenderMode::Partial {
            self.render_state = RenderState::Partial;
        } else if self.render_state != RenderState::Fully {
            self.render_state = RenderState::Fully;
            emit(events, RendererEvent::RenderMapFinished);
        }
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
    // What the layers draw before the first pass, bottom to top, such as
    // their render targets, which the passes sample, and what custom
    // layers' hosts record.
    for &index in frame.layers.keys() {
        orchestrator.layer_mut(index as usize).render_offscreen(gpu, parameters);
    }

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
    if frame.transform.view.is_globe() {
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
