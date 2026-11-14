// C++ keeps a `gfx::RendererBackend&`; the frontend owns the backend, so the
// renderer shares it through an `Rc`.

use std::mem::ManuallyDrop;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use mln_gfx::shaders::layer_ubo::GlobalPaintParamsUBO;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_defines::ID_GLOBAL_PAINT_PARAMS_UBO;
use mln_gfx::{
    BackendScope, FrameTarget, Handoff, RenderPassDescriptor, RendererBackend, ScissorRect, ScopeType, ShaderRegistry,
};
use mln_map_types::{MapDebugOptions, MapView};
use mln_style::layers::CustomLayerFrame;
use mln_text::GlyphCache;
use mln_util::Color;
use mln_util::time::Instant;

use crate::frame_projection::{self, FrameProjection};
use crate::paint_parameters::PaintParameters;
use crate::render_orchestrator::RenderOrchestrator;
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tree::RenderTree;
use crate::renderer_event::{RenderMode, RendererEvent, RendererEvents};
use crate::tile::tile_worker_transport::TileWorkerTransport;
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
    /// Updating the layer groups and their drawables from the tiles.
    pub layers: Duration,
    /// Making the shaders when there are none, running the layer tweakers,
    /// and uploading buffers and textures.
    pub upload: Duration,
    /// Encoding the render passes and submitting them.
    pub draw: Duration,
}

pub struct Renderer {
    /// Dropped by hand in `drop`, inside a `BackendScope`: its layer groups,
    /// atlases and tiles release graphics objects, which some backends (GL)
    /// can only do with their context active. A safe `Drop` runs before the
    /// fields drop, so it can't wrap their drops in the scope otherwise.
    orchestrator: ManuallyDrop<RenderOrchestrator>,
    backend: Rc<dyn RendererBackend>,
    pixel_ratio: f32,
    /// Dropped in `drop` inside the `BackendScope`, after the orchestrator.
    static_data: Option<RenderStaticData>,
    render_state: RenderState,
    frame_count: u64,
    style_loaded: bool,
    /// Where the last frame drew the map.
    frame_projection: Option<FrameProjection>,
    timings: RenderTimings,
}

impl Renderer {
    /// A renderer with a glyph cache of its own. `local_font_family` is the
    /// platform's `LocalGlyphRasterizer`'s (see `DefaultLocalGlyphRasterizer`).
    pub fn new(backend: Rc<dyn RendererBackend>, pixel_ratio: f32, local_font_family: Option<String>) -> Self {
        Self::with_glyph_cache(backend, pixel_ratio, local_font_family, Arc::default())
    }

    /// A renderer that uses `glyph_cache`: renderers given the same
    /// `GlyphCache` share the glyphs they parse from glyph PBFs.
    pub fn with_glyph_cache(
        backend: Rc<dyn RendererBackend>,
        pixel_ratio: f32,
        local_font_family: Option<String>,
        glyph_cache: Arc<GlyphCache>,
    ) -> Self {
        Renderer {
            orchestrator: ManuallyDrop::new(RenderOrchestrator::new(true, local_font_family, glyph_cache)),
            backend,
            pixel_ratio,
            static_data: None,
            render_state: RenderState::Never,
            frame_count: 0,
            style_loaded: false,
            frame_projection: None,
            timings: RenderTimings::default(),
        }
    }

    /// Draws at `pixel_ratio` device pixels per logical pixel from the next
    /// frame on, which should be the map's (`Map::set_pixel_ratio`). The
    /// renderer makes its shaders and drawables again, and keeps its
    /// tiles. The host sizes the backend's framebuffer for the new ratio.
    pub fn set_pixel_ratio(&mut self, pixel_ratio: f32) {
        let backend = Rc::clone(&self.backend);
        let _guard = BackendScope::new(&*backend, ScopeType::Explicit);
        if pixel_ratio == self.pixel_ratio {
            return;
        }
        // The shaders build the pixel ratio in on some backends, so a new
        // ratio takes new shaders, and new drawables that use them.
        self.pixel_ratio = pixel_ratio;
        self.static_data = None;
        self.orchestrator.release_shaders();
    }

    #[must_use]
    pub fn get_pixel_ratio(&self) -> f32 {
        self.pixel_ratio
    }

    /// Port-only: the transport that creates the tile workers of tiles
    /// created from now on (by default on the map's thread pool). Set it
    /// before the first render so that every tile uses it.
    pub fn set_tile_worker_transport(&mut self, transport: Rc<dyn TileWorkerTransport>) {
        self.orchestrator.set_tile_worker_transport(transport);
    }

    /// Sets the queue the renderer pushes its events to: the map's, which
    /// the map's frontend passes on.
    pub fn set_event_queue(&mut self, events: Option<RendererEvents>) {
        self.orchestrator.set_event_queue(events);
    }

    /// Draws `update_parameters` into a frame of `target`, which must be
    /// built on the renderer's backend. Returns where the frame went, or
    /// `None` when nothing was drawn: there was nothing to draw, or the
    /// target had nothing to draw into.
    pub fn render(&mut self, update_parameters: &Arc<UpdateParameters>, target: &dyn FrameTarget) -> Option<Handoff> {
        let style_changed = self.style_loaded && !update_parameters.style_loaded;
        self.style_loaded = update_parameters.style_loaded;
        if style_changed {
            self.orchestrator.reset_dynamic_texture_atlas();
        }
        let start = Instant::now();
        let mut render_tree = self.orchestrator.create_render_tree(update_parameters)?;
        render_tree.prepare();
        let prepared = Instant::now();
        let placement = self.orchestrator.symbol_placement_start.unwrap_or(prepared).max(start);
        let handoff = self.render_frame(&render_tree, update_parameters, target);
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
        &self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        self.orchestrator.query_rendered_features(geometry, options)
    }

    pub fn query_rendered_features_at_point(
        &self,
        point: &mln_util::geo::ScreenCoordinate,
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        self.orchestrator.query_rendered_features(&[*point], options)
    }

    pub fn query_rendered_features_in_box(
        &self,
        min: &mln_util::geo::ScreenCoordinate,
        max: &mln_util::geo::ScreenCoordinate,
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        use mln_util::geo::ScreenCoordinate;
        self.orchestrator.query_rendered_features(
            &[*min, ScreenCoordinate::new(max.x, min.y), *max, ScreenCoordinate::new(min.x, max.y), *min],
            options,
        )
    }

    pub fn query_source_features(
        &self,
        source_id: &str,
        options: &crate::query::SourceQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        self.orchestrator.query_source_features(source_id, options)
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

    pub fn reduce_memory_use(&mut self) {
        let _guard = BackendScope::new(&*self.backend, ScopeType::Explicit);
        self.backend.get_context().reduce_memory_usage();
        self.orchestrator.reduce_memory_use();
    }

    pub fn clear_data(&mut self) {
        self.orchestrator.clear_data();
    }

    /// Draws a frame of `render_tree` into `target`, unless the target has
    /// nothing to draw into now.
    fn render_frame(
        &mut self,
        render_tree: &RenderTree,
        update_parameters: &Arc<UpdateParameters>,
        target: &dyn FrameTarget,
    ) -> Option<Handoff> {
        let start = Instant::now();
        let backend = Rc::clone(&self.backend);
        let context = backend.get_context();

        let image = target.begin_frame()?;
        context.begin_frame();

        if self.static_data.is_none() {
            let static_data = RenderStaticData::new(Box::new(ShaderRegistry::new()));

            // Initialize shaders for drawables
            let program_parameters = ProgramParameters::new(self.pixel_ratio, false);
            backend.init_shaders(&static_data.shaders, &program_parameters);

            // Notify post-shader registration
            self.static_data = Some(static_data);
        }

        let render_tree_parameters = render_tree.get_parameters();
        let static_data = self.static_data.as_mut().expect("static data");
        static_data.has_3d = render_tree_parameters.has_3d;
        let renderable_size = image.size();

        if self.render_state == RenderState::Never {
            self.orchestrator.emit(RendererEvent::RenderMapStarted);
        }

        self.orchestrator.emit(RendererEvent::RenderFrameStarted);

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
        let orchestrator = &mut self.orchestrator;

        let mut parameters = PaintParameters::new(
            context,
            self.pixel_ratio,
            &*backend,
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

        // - UPLOAD PASS ---------------------------------------------------
        // Uploads all required buffers and images before we do any actual
        // rendering.
        {
            let encoder = parameters.encoder.as_ref().expect("encoder");
            let upload_pass = encoder.create_upload_pass("upload", &*image);

            // Update all clipping IDs + upload buckets.
            for item in render_tree.get_source_render_items() {
                item.upload(&*upload_pass);
            }
            static_data.upload(&*upload_pass);
            parameters.line_atlas.borrow_mut().upload(&*upload_pass);
            parameters.pattern_atlas.borrow_mut().upload(&*upload_pass); // conformance
        }

        // - LAYER GROUP UPDATE --------------------------------------------
        // Updates all layer groups and process changes
        let uploaded = Instant::now();
        orchestrator.update_layers(
            &static_data.shaders,
            context,
            &render_tree_parameters.transform_params.state,
            update_parameters,
            &parameters,
            render_tree,
        );

        orchestrator.process_changes();
        let layers_updated = Instant::now();

        // Upload layer groups
        {
            let encoder = parameters.encoder.as_ref().expect("encoder");
            let upload_pass = encoder.create_upload_pass("layerGroup-upload", &*image);

            // Update the debug layer groups
            orchestrator.update_debug_layer_groups(render_tree, &parameters); // layers: debug

            // Tweakers are run in the upload pass so they can set up
            // uniforms.
            parameters.current_layer = 0;
            for layer_group in orchestrator.layer_groups() {
                orchestrator.run_tweakers(&layer_group, &parameters);
                parameters.current_layer += 1;
            }
            parameters.current_layer = 0;
            // layers: debug
            for layer_group in orchestrator.debug_layer_groups() {
                orchestrator.run_tweakers(&layer_group, &parameters);
                parameters.current_layer += 1;
            }

            // Give the layers a chance to upload
            orchestrator.visit_layer_groups(|layer_group| layer_group.upload(&*upload_pass));
            if let Some(globe_depth) = orchestrator.get_globe_depth_layer_group() {
                globe_depth.borrow_mut().upload(&*upload_pass);
            }
            if let Some(sky) = orchestrator.get_sky_layer_group() {
                sky.borrow_mut().upload(&*upload_pass);
            }
            if let Some(atmosphere) = orchestrator.get_atmosphere_layer_group() {
                atmosphere.borrow_mut().upload(&*upload_pass);
            }

            // Give the render targets a chance to upload
            // dem-heatmap
            for render_target in orchestrator.render_targets() {
                render_target.borrow().upload(&*upload_pass);
            }

            // Upload the Debug layer group
            // layers: debug
            for layer_group in orchestrator.debug_layer_groups() {
                layer_group.borrow_mut().upload(&*upload_pass);
            }
        }

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

        // - CUSTOM LAYER PRERENDER ------------------------------------------
        // Custom layers record their own work before the first pass, with the
        // projection that the frame's result reports. The 3D layers draw into
        // depth from 0 to the passes' depth range size, which the layer
        // groups' count sets.
        let frame_projection = frame_projection::compute(
            &render_tree_parameters.transform_params,
            1.0 - ((orchestrator.num_layer_groups() + 2) * num_sublayers) as f32 * depth_epsilon,
            context.backend_type(),
        );
        parameters.custom_layer_frame = Some(CustomLayerFrame {
            view: MapView::from_state(state, pixel_ratio),
            projection: frame_projection,
            width: renderable_size.width,
            height: renderable_size.height,
        });
        for item in render_tree.get_layer_render_item_map().values() {
            item.layer.borrow_mut().prerender(&parameters, &*image);
        }

        // - 3D PASS -------------------------------------------------------
        // Renders any 3D layers bottom-to-top to unique FBOs with texture
        // attachments, but share the same depth rbo between them.
        if parameters.static_data.has_3d {
            // common3DPass
            parameters.pass = RenderPass::Pass3D;

            // drawable3DPass: draw layer groups, 3D pass
            debug_assert_eq!(parameters.pass, RenderPass::Pass3D);
            // `depthModeFor3D` uses this, and the 3D pass runs before the opaque pass sets it
            parameters.depth_range_size =
                1.0 - ((orchestrator.num_layer_groups() + 2) * num_sublayers) as f32 * depth_epsilon;
            parameters.current_layer = (orchestrator.num_layer_groups() as u32).wrapping_sub(1);
            orchestrator.visit_layer_groups(|layer_group| {
                layer_group.render(&mut parameters);
                if parameters.current_layer > 0 {
                    parameters.current_layer -= 1;
                }
            });
        }

        // drawableTargetsPass: draw render targets
        // dem-heatmap
        for render_target in orchestrator.render_targets() {
            render_target.borrow().render(orchestrator, &mut parameters);
        }

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
            let encoder = parameters.encoder.as_ref().expect("encoder");
            parameters.render_pass = Some(encoder.create_render_pass(
                "main buffer",
                &RenderPassDescriptor {
                    target: &*image,
                    clear_color: Some(color),
                    clear_depth: Some(1.0),
                    clear_stencil: Some(0),
                },
            ));
        }
        context.bind_global_uniform_buffers(parameters.render_pass.as_deref().expect("render pass"));

        // drawableSkyPass
        if let Some(sky) = orchestrator.get_sky_layer_group() {
            parameters.pass = RenderPass::Translucent;
            parameters.depth_range_size =
                1.0 - ((orchestrator.num_layer_groups() + 2) * num_sublayers) as f32 * depth_epsilon;
            parameters.current_layer = orchestrator.num_layer_groups() as u32 + 1;
            sky.borrow_mut().render(&mut parameters);
        }

        // drawableOpaquePass
        {
            parameters.pass = RenderPass::Opaque;
            parameters.depth_range_size =
                1.0 - ((orchestrator.num_layer_groups() + 2) * num_sublayers) as f32 * depth_epsilon;

            // draw layer groups, opaque pass: front to back with the depth
            // buffer, except on the globe, where 2D layers carry no depth and
            // are painted bottom to top.
            if render_tree_parameters.transform_params.state.is_globe_rendering() {
                parameters.current_layer = (orchestrator.num_layer_groups() as u32).wrapping_sub(1);
                orchestrator.visit_layer_groups(|layer_group| {
                    layer_group.render(&mut parameters);
                    if parameters.current_layer > 0 {
                        parameters.current_layer -= 1;
                    }
                });
            } else {
                parameters.current_layer = 0;
                orchestrator.visit_layer_groups_reversed(|layer_group| {
                    layer_group.render(&mut parameters);
                    parameters.current_layer += 1;
                });
            }
        }

        // drawableTranslucentPass
        {
            parameters.pass = RenderPass::Translucent;
            parameters.depth_range_size =
                1.0 - ((orchestrator.num_layer_groups() + 2) * num_sublayers) as f32 * depth_epsilon;

            // The planet goes into the depth buffer first, so 3D geometry
            // behind the horizon stays hidden.
            if let Some(globe_depth) = orchestrator.get_globe_depth_layer_group() {
                globe_depth.borrow_mut().render(&mut parameters);
            }

            // draw layer groups, translucent pass
            parameters.current_layer = (orchestrator.num_layer_groups() as u32).wrapping_sub(1);
            orchestrator.visit_layer_groups(|layer_group| {
                layer_group.render(&mut parameters);
                if parameters.current_layer > 0 {
                    parameters.current_layer -= 1;
                }
            });
        }

        // drawableAtmospherePass
        if let Some(atmosphere) = orchestrator.get_atmosphere_layer_group() {
            parameters.pass = RenderPass::Translucent;
            parameters.depth_range_size =
                1.0 - ((orchestrator.num_layer_groups() + 2) * num_sublayers) as f32 * depth_epsilon;
            parameters.current_layer = orchestrator.num_layer_groups() as u32 + 1;
            atmosphere.borrow_mut().render(&mut parameters);
        }

        // drawableDebugOverlays
        // Renders debug overlays.
        parameters.current_layer = 0;
        // layers: debug
        for layer_group in orchestrator.debug_layer_groups() {
            layer_group.borrow_mut().render(&mut parameters);
            parameters.current_layer += 1;
        }

        // Give the layers a chance to do cleanup
        orchestrator.visit_layer_groups(|layer_group| layer_group.post_render(&mut parameters));
        context.unbind_global_uniform_buffers(parameters.render_pass.as_deref().expect("render pass"));

        // Ends the RenderPass
        parameters.render_pass = None;

        let start_rendering = Instant::now();
        // Ending the frame submits its commands.
        let handoff = target.end_frame(&**parameters.encoder.as_ref().expect("encoder"));
        let submitted = Instant::now();
        context.rendering_stats().rendering_time = (submitted - start_rendering).as_secs_f64();

        parameters.encoder = None;
        context.end_frame();

        let rendering_time = context.rendering_stats().rendering_time;
        context.rendering_stats().encoding_time = render_tree.get_elapsed_time() - rendering_time;

        let mut stats = context.thread_safe_copy_rendering_stats();
        stats.frame_rendered_features = orchestrator.move_frame_rendered_features();
        let stats = Arc::new(stats);
        drop(parameters);

        self.timings.layers = layers_updated - uploaded;
        self.timings.upload = (uploaded - start) + (layers_uploaded - layers_updated);
        self.timings.draw = submitted - layers_uploaded;

        self.orchestrator.emit(RendererEvent::RenderFrameFinished {
            mode: if render_tree_parameters.loaded { RenderMode::Full } else { RenderMode::Partial },
            needs_repaint: render_tree_parameters.needs_repaint,
            placement_changed: render_tree_parameters.placement_changed,
            still_image_request_id: if update_parameters.still_image_request {
                update_parameters.still_image_request_id
            } else {
                0
            },
            stats,
        });

        if !render_tree_parameters.loaded {
            self.render_state = RenderState::Partial;
        } else if self.render_state != RenderState::Fully {
            self.render_state = RenderState::Fully;
            self.orchestrator.emit(RendererEvent::RenderMapFinished);
        }

        self.frame_count += 1;
        self.frame_projection = Some(frame_projection);
        Some(handoff)
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // `gfx::BackendScope guard{impl->backend}; impl.reset();`
        let backend = Rc::clone(&self.backend);
        let _guard = BackendScope::new(&*backend, ScopeType::Explicit);
        // SAFETY: `orchestrator` is dropped once, here, and nothing reads it
        // afterwards: the remaining fields' drops don't touch it.
        #[expect(unsafe_code, reason = "drops the orchestrator inside the backend scope; see its field")]
        unsafe {
            ManuallyDrop::drop(&mut self.orchestrator);
        }
        self.static_data = None;
    }
}
