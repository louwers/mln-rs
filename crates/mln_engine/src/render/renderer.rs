use std::sync::Arc;
use std::time::Duration;

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, Gpu, LoadOp, TargetFormats};
use mln_gfx::rendering_stats::{RenderingStats, TerrainStats};
use mln_gfx::shaders::bindings::PAINT_PARAMS_ID;
use mln_gfx::{FrameImage, Handoff, Rgba, ScissorRect};
use mln_style::color::Color;
use mln_style::error::Error;
use mln_style::time::Instant;

use crate::camera::FrameProjection;
use crate::draw::context::Context;
use crate::draw::draws::{DrawGroups, DrawOrder};
use crate::exec::event_queue::Waker;
use crate::exec::pool::Pool;
use crate::layers::custom::end_hosts;
use crate::render::clip_masks::ClipMasks;
use crate::render::events::{RenderMode, RendererEvent, RendererEvents};
use crate::render::frame::Frame;
use crate::render::frame_projection;
use crate::render::layer_composite::LayerComposite;
use crate::render::orchestrator::RenderOrchestrator;
use crate::render::paint_parameters::{OpenPass, PaintParameters};
use crate::render::pass::RenderPass;
use crate::render::tile_matrix::to_projection_ubo;
use crate::render::update_parameters::UpdateParameters;
use crate::text::GlyphCache;
use crate::text::local_fonts::LocalGlyphs;
use crate::worker::worker_codec::RemoteWorkers;
use crate::{Registry, log_target};

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
    /// Why the frame failed on the GPU, if it did.
    error: Option<Error>,
    mode: RenderMode,
    needs_repaint: bool,
    /// When the frame's placement wants the next frame, if it does.
    repaint_at: Option<Instant>,
    placement_changed: bool,
    /// The `still_image_request_id` of a still image's frame, or 0.
    still_image_request_id: u64,
    /// The `update_id` of the update the frame drew.
    update_id: u64,
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
    /// Kept across frames; dropped in the tear-down.
    clip_masks: ClipMasks,
    /// What layers drawn as a whole draw into and composite with, between
    /// frames.
    layer_composite: LayerComposite,
    /// Whether the context's shaders and pipelines are of an earlier pixel
    /// ratio, which the next update's render layers no
    /// longer draw with.
    shaders_replaced: bool,
    render_state: RenderState,
    /// Where the last frame drew the map.
    frame_projection: Option<FrameProjection>,
    timings: RenderTimings,
}

impl DrawnFrame {
    /// Why the frame failed on the GPU, if it did. Its image then lacks what
    /// failed, or holds nothing.
    #[must_use]
    pub fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }
}

impl Renderer {
    /// A renderer with a glyph cache of its own, which draws the layer and
    /// source types of `registry`, runs its background work on `pool` and
    /// draws `local_glyphs`. Its chooser of the device's
    /// fonts runs there too, one choice at a time, never on the thread that
    /// calls the renderer.
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
    ///
    /// The map that the renderer draws must have a registry filled by the
    /// same calls as `registry`: a layer whose type `registry` lacks isn't
    /// drawn.
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
        self.replace_shaders(pixel_ratio);
    }

    /// Compiles the shaders at `pixel_ratio` from the next update on. The render layers' draws go with
    /// that update, which makes the layers again, and the old shaders after
    /// it.
    fn replace_shaders(&mut self, pixel_ratio: f32) {
        let encoder = &mut self.encoder;
        encoder.pixel_ratio = pixel_ratio;
        encoder.context.set_program_parameters(pixel_ratio);
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
    /// loaded yet. Custom layers that the update removed or gave a new host
    /// end on `gpu`.
    pub fn update(&mut self, gpu: &mut dyn Gpu, update_parameters: &Arc<UpdateParameters>) -> Option<UpdatedFrame> {
        let orchestrator = &mut self.orchestrator;
        // Whether terrain draws depends on whether the backend has its shader.
        orchestrator.terrain().make_shader(gpu, &mut self.encoder.context, update_parameters);
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
    /// ends on its target, whether or not it failed ([`DrawnFrame::error`]),
    /// and then [finishes](Self::finish_frame).
    pub fn render(&mut self, gpu: &mut dyn Gpu, image: &FrameImage, frame: UpdatedFrame) -> DrawnFrame {
        self.encoder.encode(gpu, &mut self.orchestrator, self.events.as_ref(), image, frame)
    }

    /// Ends `frame`, which [`render`](Self::render) drew, once the host's
    /// target handed it off as `handoff`: destroys what the frame replaced
    /// and reports the frame with its rendering stats, and with its error
    /// if it failed.
    pub fn finish_frame(&mut self, gpu: &mut dyn Gpu, frame: DrawnFrame, handoff: Handoff) {
        self.encoder.finish(gpu, self.events.as_ref(), frame, handoff);
    }

    /// The terrain of the frames it draws.
    #[cfg(test)]
    pub(crate) fn terrain(&mut self) -> &crate::render::terrain::RenderTerrain {
        self.orchestrator.terrain()
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
        geometry: &[crate::geo::ScreenCoordinate],
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<crate::tile::feature::Feature>, Error> {
        let features = self.orchestrator.query_rendered_features(geometry, options);
        report(&mut self.orchestrator, self.events.as_ref());
        features
    }

    pub fn query_rendered_features_at_point(
        &mut self,
        point: &crate::geo::ScreenCoordinate,
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<crate::tile::feature::Feature>, Error> {
        self.query_rendered_features(&[*point], options)
    }

    pub fn query_rendered_features_in_box(
        &mut self,
        min: &crate::geo::ScreenCoordinate,
        max: &crate::geo::ScreenCoordinate,
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<crate::tile::feature::Feature>, Error> {
        use crate::geo::ScreenCoordinate;
        self.query_rendered_features(
            &[*min, ScreenCoordinate::new(max.x, min.y), *max, ScreenCoordinate::new(min.x, max.y), *min],
            options,
        )
    }

    pub fn query_source_features(
        &mut self,
        source_id: &str,
        options: &crate::query::SourceQueryOptions,
    ) -> Result<Vec<crate::tile::feature::Feature>, Error> {
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
        feature: &crate::tile::feature::Feature,
        extension: &str,
        extension_field: &str,
        args: Option<&mln_style::feature::PropertyMap>,
    ) -> Result<crate::query::FeatureExtensionValue, Error> {
        self.orchestrator.query_feature_extensions(source_id, feature, extension, extension_field, args)
    }

    /// The data of the symbols the last placement placed, when
    /// [`collect_placed_symbol_data`](Self::collect_placed_symbol_data)
    /// turned their collection on.
    #[must_use]
    pub fn placed_symbols_data(&self) -> Vec<crate::placement::PlacedSymbolData> {
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
        state: &mln_style::feature::FeatureState,
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
    ) -> mln_style::feature::FeatureState {
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

    /// Lets go of what the orchestrator caches, destroys what it and the
    /// renderer released, and waits for `gpu`, so that its work finishes
    /// before the app goes to the background.
    pub fn reduce_memory_use(&mut self, gpu: &mut dyn Gpu) {
        self.orchestrator.reduce_memory_use();
        self.encoder.context.destroy_released(gpu);
        if let Err(error) = gpu.flush(true) {
            log::error!(target: log_target::RENDER, "Waiting for the GPU failed: {error}");
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
        report(&mut self.orchestrator, self.events.as_ref());
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
            stats.terrain = TerrainStats::default();
            stats.num_frames += 1;
        }

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
            self.pixel_ratio,
            renderable_size,
            scissor_rect,
        );

        // - UPLOAD --------------------------------------------------------
        // The tiles' atlas first, whose textures the tiles bind, then the
        // tiles' buckets.
        orchestrator.upload_atlas(gpu, parameters.context);
        orchestrator.upload_sources(parameters.context);

        // - DRAW UPDATE ---------------------------------------------------
        // The layers' draws of the frame.
        let uploaded = Instant::now();
        orchestrator.update_layer_draws(gpu, &update_parameters, &mut parameters, &frame);
        let layers_updated = Instant::now();

        // What the frame's update and draw update replaced goes before the
        // uploads make what replaces it.
        parameters.context.destroy_released(gpu);

        // - DRAW UPLOAD ---------------------------------------------------
        orchestrator.upload_draws(gpu, parameters.context);
        let draw_groups = DrawGroups::of(
            |visit| orchestrator.visit_draw_groups(visit),
            |tiles| parameters.stencil_clear_tiles(tiles),
        );
        parameters.stencil_clear.clone_from(&draw_groups.stencil_clear);
        parameters.opaque_pass_cutoff = orchestrator.opaque_pass_cutoff();
        let (proj_state, transform) = (parameters.view, parameters.transform_params);
        parameters.clip_masks.update(gpu, parameters.context, proj_state.is_globe(), &draw_groups.clip_tiles, |tile| {
            to_projection_ubo(&transform.projection_data_for_tile(tile, false))
        });
        orchestrator.terrain().upload_drapes(gpu, &mut parameters);
        parameters.layer_composite.update(
            gpu,
            parameters.context,
            image.size,
            image.targets.color,
            &draw_groups.composited,
        );
        let global_paint_params_ubo = parameters.global_paint_params();
        parameters.context.write_global_uniform_buffer(gpu, PAINT_PARAMS_ID, &global_paint_params_ubo);
        // What the uploads replaced goes before the passes make what
        // replaces it, such as a draw's geometry.
        parameters.context.destroy_released(gpu);
        let layers_uploaded = Instant::now();

        // - DRAW ----------------------------------------------------------
        // The 3D layers draw into depth from 0 to the passes' depth range
        // size, which the draw groups' count sets, and the layers that the
        // frame lends its commands draw with the projection that the frame's
        // result reports.
        let depth_range_size = draw_groups.depth_range_size();
        let clip_depth = parameters.context.caps().clip_depth;
        let frame_projection = frame_projection::compute(&frame.transform, depth_range_size, clip_depth, frame.terrain);
        parameters.lent_projection = Some(frame_projection);
        draw(orchestrator, gpu, &mut parameters, &frame, image, draw_groups.count, depth_range_size);
        orchestrator.terrain().count(parameters.context);
        let start_rendering = Instant::now();
        let submitted = parameters.end_pass(gpu).and_then(|()| gpu.flush(false));
        if let Err(error) = &submitted {
            parameters.context.fail(format!("Submitting the frame failed: {error}"));
        }
        // A target that a frame that failed drew may hold nothing.
        parameters.context.end_targets(submitted.is_ok());
        let error = parameters.context.device().take_failure().map(Error::native);
        drop(parameters);
        // A drape of a frame that failed may lack what failed: every drape
        // draws again in the next frame.
        if error.is_some() {
            orchestrator.terrain().redraw_drapes();
        }

        self.timings.layers = layers_updated - uploaded;
        self.timings.upload = (uploaded - start) + (layers_uploaded - layers_updated);
        self.frame_projection = Some(frame_projection);
        DrawnFrame {
            mode: if frame.loaded { RenderMode::Full } else { RenderMode::Partial },
            error,
            needs_repaint: frame.needs_repaint,
            repaint_at: frame.repaint_at,
            placement_changed: frame.placement_changed,
            still_image_request_id: if update_parameters.still_image_request {
                update_parameters.still_image_request_id
            } else {
                0
            },
            update_id: update_parameters.update_id,
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
    fn finish(&mut self, gpu: &mut dyn Gpu, events: Option<&RendererEvents>, frame: DrawnFrame, handoff: Handoff) {
        let DrawnFrame {
            error,
            mode,
            needs_repaint,
            repaint_at,
            placement_changed,
            still_image_request_id,
            update_id,
            started,
            symbols_started,
            updated,
            layers_uploaded,
            start_rendering,
        } = frame;
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
                update_id,
                stats,
                error: error.map(Box::new),
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
    // The ground's depth, which the layers that stand on terrain hide
    // behind, in a pass of its own.
    parameters.pass = RenderPass::Translucent;
    parameters.depth_range_size = depth_range_size;
    orchestrator.terrain().render_ground(gpu, parameters);

    // The layers' targets, which the passes sample, and what the layers do
    // with the frame's commands before its passes, bottom to top; then the
    // terrain's drapes, each after its layers' targets.
    orchestrator.visit_render_layers(|layer| layer.before_passes(gpu, parameters));
    orchestrator.terrain().render_drapes(gpu, parameters);

    // The frame's pass, cleared to the backdrop, which shows where there are
    // no tiles.
    let Color { r, g, b, a, .. } = frame.background_color;
    let color = Rgba::new(r, g, b, a);
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
        parameters.context.fail(format!("Beginning the frame's pass failed: {error}"));
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
    // 3D geometry behind the horizon stays hidden. Over terrain, each stack
    // of draped layers draws its surface where its first layer draws, and
    // the surface's depth goes first where a 3D layer draws before that.
    parameters.pass = RenderPass::Translucent;
    parameters.depth_range_size = depth_range_size;
    orchestrator.globe_depth().render(gpu, parameters);
    orchestrator.terrain().render_depth(gpu, parameters);
    parameters.current_layer = (num_draw_groups as u32).wrapping_sub(1);
    orchestrator.render_draw_groups(gpu, parameters, DrawOrder::BottomUp);

    // The atmosphere.
    if let Some(atmosphere) = orchestrator.atmosphere() {
        parameters.pass = RenderPass::Translucent;
        parameters.depth_range_size = depth_range_size;
        parameters.current_layer = num_draw_groups as u32 + 1;
        atmosphere.render(gpu, parameters);
    }

    // The frame, where it drew into the layer composite's texture, copied
    // into the target.
    let present = OpenPass {
        label: "main buffer",
        color: Some(ColorAttachment { texture: image.color, load: LoadOp::Clear(Rgba::new(0.0, 0.0, 0.0, 0.0)) }),
        depth_stencil: None,
        size: image.size,
        targets: TargetFormats { color: image.targets.color, depth_stencil: None },
    };
    if let Err(error) = parameters.present_frame(gpu, present) {
        parameters.context.fail(format!("Copying the frame into its target failed: {error}"));
    }
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
