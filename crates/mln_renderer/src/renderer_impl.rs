// Ports src/mln/renderer/renderer_impl.hpp and src/mln/renderer/renderer_impl.cpp
// (the non-OpenGL branches). The Metal branch's autorelease pool is the Metal
// backend's (mln_mtl renderer_backend.rs), and its frame capture, off in
// C++, is not ported.

use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::shaders::layer_ubo::GlobalPaintParamsUBO;
use mln_gfx::shaders::program_parameters::ProgramParameters;
use mln_gfx::shaders::shader_defines::ID_GLOBAL_PAINT_PARAMS_UBO;
use mln_gfx::{FrameTarget, Handoff, RenderPassDescriptor, RendererBackend, ScissorRect, ShaderRegistry};
use mln_map_types::{MapDebugOptions, MapView};
use mln_style::layers::CustomLayerFrame;
use mln_text::GlyphCache;
use mln_util::Color;

use crate::frame_projection::{self, FrameProjection};
use crate::paint_parameters::PaintParameters;
use crate::render_orchestrator::RenderOrchestrator;
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tree::RenderTree;
use crate::renderer_event::{RenderMode, RendererEvent};
use crate::update_parameters::UpdateParameters;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RenderState {
    Never,
    Partial,
    Fully,
}

pub struct RendererImpl {
    pub orchestrator: RenderOrchestrator,
    pub backend: Rc<dyn RendererBackend>,
    pixel_ratio: f32,
    static_data: Option<RenderStaticData>,
    render_state: RenderState,
    frame_count: u64,
    pub style_loaded: bool,
    /// Where the last frame drew the map.
    frame_projection: Option<FrameProjection>,
    // TODO(port): dynamicTextureAtlas (symbols, M5).
}

impl RendererImpl {
    /// `Renderer::Impl(gfx::RendererBackend&, float pixelRatio,
    /// const std::optional<std::string>& localFontFamily,
    /// std::shared_ptr<GlyphCache>)`.
    pub fn new(
        backend: Rc<dyn RendererBackend>,
        pixel_ratio: f32,
        local_font_family: Option<String>,
        glyph_cache: Arc<GlyphCache>,
    ) -> Self {
        RendererImpl {
            orchestrator: RenderOrchestrator::new(true, local_font_family, glyph_cache),
            backend,
            pixel_ratio,
            static_data: None,
            render_state: RenderState::Never,
            frame_count: 0,
            style_loaded: false,
            frame_projection: None,
        }
    }

    pub fn pixel_ratio(&self) -> f32 {
        self.pixel_ratio
    }

    /// The shaders build the pixel ratio in on some backends, so a new
    /// ratio takes new shaders, and new drawables that use them.
    pub fn set_pixel_ratio(&mut self, pixel_ratio: f32) {
        if pixel_ratio == self.pixel_ratio {
            return;
        }
        self.pixel_ratio = pixel_ratio;
        self.static_data = None;
        self.orchestrator.release_shaders();
    }

    /// Draws a frame of `render_tree` into `target`, unless the target has
    /// nothing to draw into now.
    pub fn render(
        &mut self,
        render_tree: &RenderTree,
        update_parameters: &Arc<UpdateParameters>,
        target: &dyn FrameTarget,
    ) -> Option<Handoff> {
        let backend = self.backend.clone();
        let context = backend.get_context();
        // TODO(port): context.setObserver(this) (shader compile callbacks).

        let image = target.begin_frame()?;
        context.begin_frame();

        if self.static_data.is_none() {
            let static_data = RenderStaticData::new(Box::new(ShaderRegistry::new()));

            // Initialize shaders for drawables
            let program_parameters = ProgramParameters::new(self.pixel_ratio, false);
            backend.init_shaders(&static_data.shaders, &program_parameters);

            // Notify post-shader registration
            // TODO(port): observer->onRegisterShaders(*staticData->shaders).
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
            render_tree_parameters.map_mode,
            render_tree_parameters.debug_options,
            render_tree_parameters.time_point,
            &render_tree_parameters.transform_params,
            static_data,
            orchestrator.get_pattern_atlas().clone(),
            self.frame_count,
            update_parameters.tile_lod_min_radius,
            update_parameters.tile_lod_scale,
            update_parameters.tile_lod_pitch_threshold,
            update_parameters.tile_lod_mode,
            renderable_size,
            scissor_rect,
            update_parameters.capture_rendered_features,
        );

        parameters.line_atlas = orchestrator.get_line_atlas().clone();
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
            // TODO(port): patternAtlas.upload (patterns).
        }

        // - LAYER GROUP UPDATE --------------------------------------------
        // Updates all layer groups and process changes
        orchestrator.update_layers(
            &static_data.shaders,
            context,
            &render_tree_parameters.transform_params.state,
            update_parameters,
            &parameters,
            render_tree,
        );

        orchestrator.process_changes();

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

        let start_rendering = mln_util::time::Instant::now();
        // Ending the frame submits its commands.
        let handoff = target.end_frame(&**parameters.encoder.as_ref().expect("encoder"));
        context.rendering_stats().rendering_time = start_rendering.elapsed().as_secs_f64();

        parameters.encoder = None;
        context.end_frame();

        let rendering_time = context.rendering_stats().rendering_time;
        context.rendering_stats().encoding_time = render_tree.get_elapsed_time() - rendering_time;

        let mut stats = context.thread_safe_copy_rendering_stats();
        stats.frame_rendered_features = orchestrator.move_frame_rendered_features();
        let stats = Arc::new(stats);
        drop(parameters);

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

    /// Where the last frame drew the map, or `None` before the first.
    #[must_use]
    pub fn frame_projection(&self) -> Option<FrameProjection> {
        self.frame_projection
    }

    /// Tells the hosts of the custom layers that the renderer's graphics
    /// objects are lost, and lets go of them, as when the session's device
    /// goes away before the renderer.
    pub fn lose_custom_layers(&mut self) {
        self.orchestrator.visit_render_layers(|layer| layer.lose_context());
    }

    pub fn reduce_memory_use(&mut self) {
        self.backend.get_context().reduce_memory_usage();
    }
}
