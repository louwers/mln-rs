// C++ keeps a `gfx::RendererBackend&`; the frontend owns the backend, so the
// renderer shares it through an `Rc`.

use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mln_gfx::gpu::{ColorAttachment, DepthStencilAttachment, LoadOp, TargetFormats};
use mln_gfx::rendering_stats::RenderingStats;
use mln_gfx::shaders::layer_ubo::GlobalPaintParamsUBO;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_defines::ID_GLOBAL_PAINT_PARAMS_UBO;
use mln_gfx::{FrameTarget, Handoff, RendererBackend, ScissorRect, ShaderRegistry};
use mln_map_types::{MapDebugOptions, MapView};
use mln_style::layers::CustomLayerFrame;
use mln_text::GlyphCache;
use mln_tile_id::UnwrappedTileID;
use mln_util::Color;
use mln_util::event_queue::Waker;
use mln_util::logging::{Event, Log};
use mln_util::pool::Pool;
use mln_util::time::Instant;

use crate::Registry;
use crate::clip_masks::ClipMasks;
use crate::draw_cache::{DrawGroup, DrawOrder};
use crate::frame_projection::{self, FrameProjection};
use crate::gfx::context::{Context, Stats};
use crate::layer_composite::LayerComposite;
use crate::paint_parameters::{OpenPass, PaintParameters};
use crate::render_orchestrator::RenderOrchestrator;
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tree::RenderTree;
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

/// Port-only: what the last frame that [`Renderer::render`] drew spent on
/// each of its steps, on the CPU. They follow each other, and together take
/// nearly all of `render`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderTimings {
    /// Updating the render layers and sources from the update: the sources'
    /// tile pyramids and the tiles' matrices.
    pub sources: Duration,
    /// Placing symbols across tiles and updating their opacities.
    pub symbols: Duration,
    /// Making the layers' draws from the tiles, with their uniforms.
    pub layers: Duration,
    /// Making the shaders when there are none, and uploading buffers and
    /// textures.
    pub upload: Duration,
    /// Encoding the render passes and submitting them.
    pub draw: Duration,
}

pub struct Renderer {
    /// Dropped by hand in `drop`, inside the backend's hold: its custom
    /// layers' hosts free their graphics objects as the layers go, which on
    /// GL needs the context current. A safe `Drop` runs before the fields
    /// drop, so it can't wrap their drops in the hold otherwise.
    orchestrator: ManuallyDrop<RenderOrchestrator>,
    backend: Rc<dyn RendererBackend>,
    /// What the renderer keeps over the backend's `Gpu`, which goes after
    /// the resources that `drop` drops first.
    context: Rc<Context>,
    pixel_ratio: f32,
    /// Dropped in `drop` inside the hold, after the orchestrator.
    static_data: Option<RenderStaticData>,
    /// Kept across frames; dropped in `drop` inside the hold.
    clip_masks: ClipMasks,
    /// What layers drawn as a whole draw into and composite with, between
    /// frames.
    layer_composite: LayerComposite,
    render_state: RenderState,
    frame_count: u64,
    /// Where the last frame drew the map.
    frame_projection: Option<FrameProjection>,
    timings: RenderTimings,
    /// Where the renderer reports its events, once the host gives it one.
    events: Option<RendererEvents>,
}

impl Renderer {
    /// A renderer with a glyph cache of its own, which runs its background
    /// work on `pool`. `local_font_family` is the platform's
    /// `LocalGlyphRasterizer`'s (see `DefaultLocalGlyphRasterizer`).
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
        local_font_family: Option<String>,
        waker: Waker,
        pool: &Pool,
    ) -> Self {
        Self::with_glyph_cache(backend, pixel_ratio, local_font_family, Arc::default(), waker, pool)
    }

    /// A renderer that uses `glyph_cache`: renderers given the same
    /// `GlyphCache` share the glyphs they parse from glyph PBFs.
    pub fn with_glyph_cache(
        backend: Rc<dyn RendererBackend>,
        pixel_ratio: f32,
        local_font_family: Option<String>,
        glyph_cache: Arc<GlyphCache>,
        waker: Waker,
        pool: &Pool,
    ) -> Self {
        let engine = crate::text::font_face_engine::default_engine();
        let registry = Registry::with_builtins();
        Self::with_fonts(backend, pixel_ratio, local_font_family, glyph_cache, engine, registry, waker, pool)
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
        local_font_family: Option<String>,
        glyph_cache: Arc<GlyphCache>,
        font_face_engine: Arc<dyn crate::text::font_face_engine::FontFaceEngine>,
        registry: Arc<Registry>,
        waker: Waker,
        pool: &Pool,
    ) -> Self {
        Renderer {
            orchestrator: ManuallyDrop::new(RenderOrchestrator::new(
                true,
                local_font_family,
                glyph_cache,
                font_face_engine,
                registry,
                waker,
                pool,
            )),
            context: Rc::new(Context::new(Rc::clone(&backend))),
            backend,
            pixel_ratio,
            static_data: None,
            clip_masks: ClipMasks::default(),
            layer_composite: LayerComposite::default(),
            render_state: RenderState::Never,
            frame_count: 0,
            frame_projection: None,
            timings: RenderTimings::default(),
            events: None,
        }
    }

    /// Draws at `pixel_ratio` device pixels per logical pixel from the next
    /// frame on, which should be the map's (`Map::set_pixel_ratio`). The
    /// renderer makes its shaders and draws again, and keeps its
    /// tiles. The host sizes the backend's framebuffer for the new ratio.
    pub fn set_pixel_ratio(&mut self, pixel_ratio: f32) {
        if pixel_ratio == self.pixel_ratio {
            return;
        }
        // The shaders build the pixel ratio in on some backends, so a new
        // ratio takes new shaders, and new draws that use them.
        self.pixel_ratio = pixel_ratio;
        self.static_data = None;
        self.clip_masks = ClipMasks::default();
        self.layer_composite = LayerComposite::default();
        self.orchestrator.release_shaders();
    }

    #[must_use]
    pub fn get_pixel_ratio(&self) -> f32 {
        self.pixel_ratio
    }

    /// Port-only: runs the tiles' jobs, and the raster tiles' workers, in
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
        self.report_orchestrator_events();
    }

    /// Sets the queue the renderer pushes its events to: the map's, which
    /// the map's frontend passes on.
    pub fn set_event_queue(&mut self, events: Option<RendererEvents>) {
        self.events = events;
    }

    /// Pushes what the orchestrator reported since the last call to the
    /// renderer's event queue, or drops it when there is no queue.
    fn report_orchestrator_events(&mut self) {
        for event in self.orchestrator.take_events() {
            emit(self.events.as_ref(), event);
        }
    }

    /// Draws `update_parameters` into a frame of `target`, which must be
    /// built on the renderer's backend. Returns where the frame went, or
    /// `None` when nothing was drawn: there was nothing to draw, or the
    /// target had nothing to draw into.
    pub fn render(&mut self, update_parameters: &Arc<UpdateParameters>, target: &dyn FrameTarget) -> Option<Handoff> {
        // Held from the update on: a custom layer that the update removes or
        // gives a new host deinitializes on the backend, as on OpenGL with
        // the context current.
        let backend = Rc::clone(&self.backend);
        let _hold = backend.hold();
        let start = Instant::now();
        let frame_layers = self.orchestrator.update(update_parameters);
        let render_tree = frame_layers.map(|layers| self.orchestrator.prepare(update_parameters, layers));
        self.report_orchestrator_events();
        let render_tree = render_tree?;
        let prepared = Instant::now();
        let placement = render_tree.symbols_start().max(start);
        let handoff = self.encode(&render_tree, update_parameters, target);
        if handoff.is_some() {
            self.timings.sources = placement - start;
            self.timings.symbols = prepared - placement;
        }
        handoff
    }

    /// Port-only: drops `value` on the renderer's workers instead of this
    /// thread, for data that would take a frame long to free, such as the
    /// sources of an earlier update. The renderer's drop waits for it.
    pub fn drop_on_workers<T: Send + 'static>(&self, value: T) {
        self.orchestrator.drop_on_workers(value);
    }

    /// Port-only: what the last frame that [`render`](Self::render) drew
    /// spent on each of its steps.
    #[must_use]
    pub fn render_timings(&self) -> RenderTimings {
        self.timings
    }

    /// Where the last frame that [`render`](Self::render) drew put the map,
    /// or `None` before the first.
    #[must_use]
    pub fn frame_projection(&self) -> Option<FrameProjection> {
        self.frame_projection
    }

    // query: `Err` is the exception C++ throws out of the query (a
    // malformed tile).
    pub fn query_rendered_features(
        &mut self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        let features = self.orchestrator.query_rendered_features(geometry, options);
        self.report_orchestrator_events();
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
        self.report_orchestrator_events();
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

    #[must_use]
    pub fn get_placed_symbols_data(&self) -> Vec<crate::text::placement::PlacedSymbolData> {
        self.orchestrator.get_placed_symbols_data()
    }

    /// Logs every render source's tiles and the image manager's state.
    pub fn dump_debug_logs(&self) {
        self.orchestrator.dump_debug_logs();
    }

    // layers: feature state
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

    #[must_use]
    pub fn get_feature_state(
        &self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
    ) -> mln_style::expression::expression::FeatureState {
        self.orchestrator.get_feature_state(source_id, source_layer_id, feature_id)
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

    #[must_use]
    pub fn get_tile_cache_enabled(&self) -> bool {
        self.orchestrator.get_tile_cache_enabled()
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
        let backend = Rc::clone(&self.backend);
        let _hold = backend.hold();
        self.context.destroy_released();
        if let Err(error) = self.context.gpu().flush(true) {
            Log::error(Event::Render, format!("Waiting for the GPU failed: {error}"));
        }
        self.orchestrator.reduce_memory_use();
        self.report_orchestrator_events();
    }

    /// The rendering stats, which count the renderer's resources until it
    /// releases them.
    #[must_use]
    pub fn rendering_stats(&self) -> RenderingStats {
        self.context.stats().get()
    }

    /// The rendering stats, to read after the renderer goes: what it
    /// releases as it goes comes off them.
    #[must_use]
    pub fn stats(&self) -> Rc<Stats> {
        Rc::clone(self.context.stats())
    }

    pub fn clear_data(&mut self) {
        // The custom layers that go deinitialize on the backend.
        let _hold = self.backend.hold();
        self.orchestrator.clear_data();
    }

    /// The last phase of a frame: draws `render_tree` into `target`, unless
    /// the target has nothing to draw into now.
    fn encode(
        &mut self,
        render_tree: &RenderTree,
        update_parameters: &Arc<UpdateParameters>,
        target: &dyn FrameTarget,
    ) -> Option<Handoff> {
        let start = Instant::now();
        let context = Rc::clone(&self.context);
        let context = &*context;

        let image = target.begin_frame()?;
        {
            let mut stats = context.rendering_stats();
            stats.num_draw_calls = 0;
            stats.num_frames += 1;
        }

        if self.static_data.is_none() {
            let program_parameters = ProgramParameters::new(self.pixel_ratio, false);
            self.static_data = Some(RenderStaticData::new(Box::new(ShaderRegistry::new(&program_parameters))));
        }

        let render_tree_parameters = render_tree.get_parameters();
        let static_data = self.static_data.as_mut().expect("static data");
        static_data.has_3d = render_tree_parameters.has_3d;
        let renderable_size = image.size;

        if self.render_state == RenderState::Never {
            emit(self.events.as_ref(), RendererEvent::RenderMapStarted);
        }

        emit(self.events.as_ref(), RendererEvent::RenderFrameStarted);

        let state = &render_tree_parameters.transform_params.state;
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

        let static_data: &RenderStaticData = static_data;
        let orchestrator = &mut *self.orchestrator;

        let mut parameters = PaintParameters::new(
            context,
            self.pixel_ratio,
            render_tree_parameters.debug_options,
            &render_tree_parameters.transform_params,
            static_data,
            Rc::clone(orchestrator.get_pattern_atlas()),
            renderable_size,
            scissor_rect,
        );

        parameters.line_atlas = Rc::clone(orchestrator.get_line_atlas());
        parameters.symbol_fade_change = render_tree_parameters.symbol_fade_change;
        parameters.evaluated_light = render_tree_parameters.light.clone(); // conformance
        parameters.opaque_pass_cutoff = render_tree_parameters.opaque_pass_cut_off;

        // - UPLOAD --------------------------------------------------------
        // Uploads all required buffers and images before we do any actual
        // rendering. The atlas goes first: the tiles bind its textures.
        orchestrator.upload_atlas(context);
        orchestrator.upload_sources(context);
        static_data.upload(context);
        parameters.line_atlas.borrow_mut().upload(context);
        parameters.pattern_atlas.borrow_mut().upload(context); // conformance

        // - DRAW UPDATE ---------------------------------------------------
        // Makes the layers' draws of the frame
        let uploaded = Instant::now();
        orchestrator.update_layers(update_parameters, &parameters, render_tree);

        let layers_updated = Instant::now();

        // - DRAW UPLOAD -----------------------------------------------------
        // Update the debug overlays
        orchestrator.update_debug_layers(&parameters); // layers: debug
        // What the update replaced, before the uploads make what replaces it.
        context.destroy_released();

        // Give the layers a chance to upload
        orchestrator.upload_draw_groups(context);
        orchestrator.globe_depth().draws.upload(context);
        if let Some(sky) = orchestrator.sky() {
            sky.upload(context);
        }
        if let Some(atmosphere) = orchestrator.atmosphere() {
            atmosphere.upload(context);
        }

        // Give the render targets a chance to upload
        // dem-heatmap
        orchestrator.visit_render_layers(|layer| layer.upload_render_targets(context));

        // Upload the debug overlays
        // layers: debug
        for debug in orchestrator.debug_layers() {
            debug.draws.upload(context);
        }

        // The masks of the tiles the draw groups clip to and, when any of
        // them uses the stencil buffer, of the tile that clears it. Draw
        // groups of one source clip to the same tiles, which are added once.
        let mut clip_tiles = Vec::new();
        let mut last: Option<&[UnwrappedTileID]> = None;
        for group in orchestrator.draw_groups() {
            if let Some(tiles) = group.stencil_tiles()
                && last.is_none_or(|last| last != tiles)
            {
                clip_tiles.extend(tiles.iter().copied());
                last = Some(tiles);
            }
        }
        if last.is_some() {
            clip_tiles.extend(parameters.stencil_clear_tile());
        }
        if !clip_tiles.is_sorted() {
            clip_tiles.sort_unstable();
        }
        clip_tiles.dedup();
        self.clip_masks.update(
            context,
            &static_data.shaders,
            parameters.state.is_globe_rendering(),
            &clip_tiles,
            |tile| to_projection_ubo(&parameters.projection_data_for_tile(tile, false)),
        );
        parameters.clip_masks = std::mem::take(&mut self.clip_masks);

        // The draw groups drawn as a whole, each into the layer composite's
        // texture and then onto the frame.
        let composited: Vec<_> = orchestrator.draw_groups().filter_map(DrawGroup::composite).collect();
        self.layer_composite.update(
            context,
            &static_data.shaders,
            image.size,
            image.targets.color,
            parameters.debug_options.contains(MapDebugOptions::OVERDRAW),
            &composited,
        );
        parameters.layer_composite = std::mem::take(&mut self.layer_composite);

        let layers_uploaded = Instant::now();
        let atlas_size = parameters.pattern_atlas.borrow().get_pixel_size();
        let world_size = parameters.renderable_size;
        let global_paint_params_ubo = GlobalPaintParamsUBO {
            pattern_atlas_texsize: [atlas_size.width as f32, atlas_size.height as f32],
            units_to_pixels: [1.0 / parameters.pixels_to_gl_units[0], 1.0 / parameters.pixels_to_gl_units[1]],
            world_size: [world_size.width as f32, world_size.height as f32],
            camera_to_center_distance: parameters.state.get_camera_to_center_distance(),
            symbol_fade_change: parameters.symbol_fade_change,
            aspect_ratio: parameters.state.get_size().aspect_ratio(),
            pixel_ratio: parameters.pixel_ratio,
            map_zoom: parameters.state.get_zoom() as f32,
            pad1: 0.0,
        };
        context.mutable_global_uniform_buffers().create_or_update_pod(
            ID_GLOBAL_PAINT_PARAMS_UBO,
            &global_paint_params_ubo,
            context,
        );

        let num_sublayers = PaintParameters::NUM_SUBLAYERS as usize;
        let depth_epsilon = PaintParameters::DEPTH_EPSILON;
        let num_draw_groups = orchestrator.num_draw_groups();

        // - CUSTOM LAYER PRERENDER ------------------------------------------
        // Custom layers record their own work before the first pass, with the
        // projection that the frame's result reports. The 3D layers draw into
        // depth from 0 to the passes' depth range size, which the draw
        // groups' count sets.
        let frame_projection = frame_projection::compute(
            &render_tree_parameters.transform_params,
            1.0 - ((num_draw_groups + 2) * num_sublayers) as f32 * depth_epsilon,
            context.caps().clip_depth,
        );
        parameters.custom_layer_frame = Some(CustomLayerFrame {
            view: MapView::from_state(state, pixel_ratio),
            projection: frame_projection,
            width: renderable_size.width,
            height: renderable_size.height,
        });
        for &index in render_tree.get_layer_render_item_map().keys() {
            orchestrator.layer_mut(index as usize).prerender(&parameters);
        }

        // - 3D PASS -------------------------------------------------------
        // Renders any 3D layers bottom-to-top to unique FBOs with texture
        // attachments, but share the same depth rbo between them. No pass is
        // open, so its draw groups draw nothing.
        if parameters.static_data.has_3d {
            // common3DPass
            parameters.pass = RenderPass::Pass3D;

            // The 3D pass: draw the draw groups.
            debug_assert_eq!(parameters.pass, RenderPass::Pass3D);
            // `depthModeFor3D` uses this, and the 3D pass runs before the opaque pass sets it
            parameters.depth_range_size = 1.0 - ((num_draw_groups + 2) * num_sublayers) as f32 * depth_epsilon;
            parameters.current_layer = (num_draw_groups as u32).wrapping_sub(1);
            let mut gpu = context.gpu();
            orchestrator.render_draw_groups(&mut *gpu, &mut parameters, DrawOrder::BottomUp);
        }

        // Draw the render targets.
        // dem-heatmap
        orchestrator.visit_render_layers(|layer| layer.render_targets(&mut parameters));

        let mut gpu = context.gpu();
        // commonClearPass
        // - CLEAR ---------------------------------------------------------
        // Renders the backdrop of the OpenGL view. This also paints in areas
        // where we don't have any tiles whatsoever.
        {
            let color = if parameters.debug_options.contains(MapDebugOptions::OVERDRAW) {
                Color::black()
            } else {
                render_tree_parameters.background_color
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
            if let Err(error) = parameters.begin_pass(&mut *gpu, pass) {
                Log::error(Event::Render, format!("Beginning the frame's pass failed: {error}"));
            }
        }

        // The sky.
        if let Some(sky) = orchestrator.sky() {
            parameters.pass = RenderPass::Translucent;
            parameters.depth_range_size = 1.0 - ((num_draw_groups + 2) * num_sublayers) as f32 * depth_epsilon;
            parameters.current_layer = num_draw_groups as u32 + 1;
            sky.render(&mut *gpu, &mut parameters);
        }

        // The opaque pass.
        {
            parameters.pass = RenderPass::Opaque;
            parameters.depth_range_size = 1.0 - ((num_draw_groups + 2) * num_sublayers) as f32 * depth_epsilon;

            // draw the draw groups, opaque pass: front to back with the depth
            // buffer, except on the globe, where 2D layers carry no depth and
            // are painted bottom to top.
            if render_tree_parameters.transform_params.state.is_globe_rendering() {
                parameters.current_layer = (num_draw_groups as u32).wrapping_sub(1);
                orchestrator.render_draw_groups(&mut *gpu, &mut parameters, DrawOrder::BottomUp);
            } else {
                parameters.current_layer = 0;
                orchestrator.render_draw_groups(&mut *gpu, &mut parameters, DrawOrder::TopDown);
            }
        }

        // The translucent pass.
        {
            parameters.pass = RenderPass::Translucent;
            parameters.depth_range_size = 1.0 - ((num_draw_groups + 2) * num_sublayers) as f32 * depth_epsilon;

            // The planet goes into the depth buffer first, so 3D geometry
            // behind the horizon stays hidden.
            orchestrator.globe_depth().draws.render(&mut *gpu, &mut parameters);

            // draw the draw groups, translucent pass
            parameters.current_layer = (num_draw_groups as u32).wrapping_sub(1);
            orchestrator.render_draw_groups(&mut *gpu, &mut parameters, DrawOrder::BottomUp);
        }

        // The atmosphere.
        if let Some(atmosphere) = orchestrator.atmosphere() {
            parameters.pass = RenderPass::Translucent;
            parameters.depth_range_size = 1.0 - ((num_draw_groups + 2) * num_sublayers) as f32 * depth_epsilon;
            parameters.current_layer = num_draw_groups as u32 + 1;
            atmosphere.render(&mut *gpu, &mut parameters);
        }

        // The debug overlays.
        // Renders debug overlays.
        parameters.current_layer = 0;
        // layers: debug
        for debug in orchestrator.debug_layers() {
            debug.draws.render(&mut *gpu, &mut parameters);
            parameters.current_layer += 1;
        }

        parameters.present_frame(
            &mut *gpu,
            OpenPass {
                label: "main buffer",
                color: Some(ColorAttachment {
                    texture: image.color,
                    load: LoadOp::Clear(Color::new(0.0, 0.0, 0.0, 0.0)),
                }),
                depth_stencil: None,
                size: image.size,
                targets: TargetFormats { color: image.targets.color, depth_stencil: None },
            },
        );

        let start_rendering = Instant::now();
        if let Err(error) = parameters.end_pass(&mut *gpu).and_then(|()| gpu.flush(false)) {
            Log::error(Event::Render, format!("Submitting the frame failed: {error}"));
        }
        drop(gpu);
        let handoff = target.end_frame();
        // The frame read back counts as a framebuffer, as C++'s headless
        // backend's offscreen texture does.
        context.rendering_stats().num_frame_buffers = i32::from(handoff == Handoff::ReadBack);
        let submitted = Instant::now();
        context.rendering_stats().rendering_time = (submitted - start_rendering).as_secs_f64();

        context.destroy_released();
        self.clip_masks = std::mem::take(&mut parameters.clip_masks);
        self.layer_composite = std::mem::take(&mut parameters.layer_composite);

        let rendering_time = context.rendering_stats().rendering_time;
        context.rendering_stats().encoding_time = render_tree.get_elapsed_time() - rendering_time;

        let stats = Arc::new(context.stats().get());
        drop(parameters);

        self.timings.layers = layers_updated - uploaded;
        self.timings.upload = (uploaded - start) + (layers_uploaded - layers_updated);
        self.timings.draw = submitted - layers_uploaded;

        emit(
            self.events.as_ref(),
            RendererEvent::RenderFrameFinished {
                mode: if render_tree_parameters.loaded { RenderMode::Full } else { RenderMode::Partial },
                needs_repaint: render_tree_parameters.needs_repaint,
                placement_changed: render_tree_parameters.placement_changed,
                still_image_request_id: if update_parameters.still_image_request {
                    update_parameters.still_image_request_id
                } else {
                    0
                },
                stats,
            },
        );

        if !render_tree_parameters.loaded {
            self.render_state = RenderState::Partial;
        } else if self.render_state != RenderState::Fully {
            self.render_state = RenderState::Fully;
            emit(self.events.as_ref(), RendererEvent::RenderMapFinished);
        }

        self.frame_count += 1;
        self.frame_projection = Some(frame_projection);
        Some(handoff)
    }
}

/// Pushes `event` to `events`, the renderer's event queue, if it has one.
fn emit(events: Option<&RendererEvents>, event: RendererEvent) {
    if let Some(events) = events {
        events.push(event);
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        let backend = Rc::clone(&self.backend);
        let _hold = backend.hold();
        // SAFETY: `orchestrator` is dropped once, here, and nothing reads it
        // afterwards: the remaining fields' drops don't touch it.
        #[expect(unsafe_code, reason = "drops the orchestrator inside the backend's hold; see its field")]
        unsafe {
            ManuallyDrop::drop(&mut self.orchestrator);
        }
        self.clip_masks = ClipMasks::default();
        self.layer_composite = LayerComposite::default();
        self.static_data = None;
        self.context.destroy_released();
    }
}
