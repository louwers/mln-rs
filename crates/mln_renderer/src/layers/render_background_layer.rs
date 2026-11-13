// Ports src/mln/renderer/layers/render_background_layer.hpp and
// src/mln/renderer/layers/render_background_layer.cpp (drawable path).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::shader_defines::ID_BACKGROUND_POS_VERTEX_ATTRIBUTE;
use mln_gfx::{AttributeDataType, ColorMode, Context, DepthMaskType, DrawMode, ShaderProgramBasePtr, ShaderRegistry};
use mln_map_types::TransformState;
use mln_style::layer::downcast_arc;
use mln_style::layer_properties::LayerProperties;
use mln_style::layers::background_layer_impl::Impl as BackgroundLayerImpl;
use mln_style::layers::{
    BackgroundColor, BackgroundLayerProperties, BackgroundOpacity, BackgroundPaintProperties, BackgroundPattern,
};
use mln_style::{PropertyEvaluationParameters, TransitionParameters, layer};
use mln_util::Color;
use mln_util::range::Range;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use super::{background_crossfade, background_layer_properties};
use crate::change_request::{ChangeRequest, UniqueChangeRequestVec};
use crate::globe_tile_mesh::raw_globe_tile_mesh;
use crate::layer_tweaker::{LayerTweakerPtr, layer_tweaker_slot};
use crate::layers::background_layer_tweaker::BackgroundLayerTweaker;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{LayerPrepareParameters, RenderLayer, RenderLayerBase};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tree::RenderTree;
use crate::update_parameters::UpdateParameters;
use crate::util::tile_cover::{TileCoverParameters, tile_cover};

fn impl_cast(impl_: &Arc<dyn layer::Impl>) -> &BackgroundLayerImpl {
    debug_assert!(std::ptr::eq(impl_.get_type_info(), BackgroundLayerImpl::static_type_info()));
    impl_.downcast_ref::<BackgroundLayerImpl>().expect("RenderBackgroundLayer: not a background layer impl")
}

const BACKGROUND_PLAIN_SHADER_NAME: &str = "BackgroundShader";
const BACKGROUND_PATTERN_SHADER_NAME: &str = "BackgroundPatternShader";

/// `addPatternIfNeeded(id, params)` (conformance).
fn add_pattern_if_needed(id: &str, params: &LayerPrepareParameters<'_>) {
    if params.pattern_atlas.borrow().get_pattern(id).is_none()
        && let Some(image) = params.image_manager.get_image(id)
    {
        params.pattern_atlas.borrow_mut().add_pattern(&image);
    }
}

pub struct RenderBackgroundLayer {
    base: RenderLayerBase,
    unevaluated: BackgroundPaintProperties::Unevaluated,
    plain_shader: Option<ShaderProgramBasePtr>,
    pattern_shader: Option<ShaderProgramBasePtr>,
}

impl RenderBackgroundLayer {
    pub fn new(impl_: Arc<BackgroundLayerImpl>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Arc<dyn LayerProperties> = Arc::new(BackgroundLayerProperties::new(impl_));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();
        RenderBackgroundLayer { base, unevaluated, plain_shader: None, pattern_shader: None }
    }

    /// The `drawPasses` / `passes` expression shared by `evaluate` and
    /// `update`.
    fn passes_for(&self, evaluated: &BackgroundPaintProperties::PossiblyEvaluated) -> RenderPass {
        if *evaluated.get::<BackgroundOpacity>() == 0.0 {
            RenderPass::None
        } else if !self.unevaluated.get::<BackgroundPattern>().is_undefined()
            || *evaluated.get::<BackgroundOpacity>() < 1.0
            || evaluated.get::<BackgroundColor>().a < 1.0
        {
            RenderPass::Translucent
        } else {
            // Supply both - evaluated based on opaquePassCutoff in render().
            RenderPass::Opaque | RenderPass::Translucent
        }
    }
}

impl RenderLayer for RenderBackgroundLayer {
    fn base(&self) -> &RenderLayerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RenderLayerBase {
        &mut self.base
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.unevaluated);
        self.unevaluated = impl_cast(&self.base.base_impl).paint.transitioned(parameters, prior);
        self.base.style_dependencies = self.unevaluated.get_dependencies();
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let previous_evaluated = background_layer_properties(&self.base.evaluated_properties).clone();
        let evaluated = self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated);
        let mut properties = BackgroundLayerProperties::with_evaluated(
            downcast_arc::<BackgroundLayerImpl>(&self.base.base_impl),
            parameters.get_crossfade_parameters(),
            evaluated,
        );

        self.base.passes = self.passes_for(&properties.evaluated);
        properties.base.render_passes = self.base.passes.underlying_type();

        self.base.evaluated_properties = Arc::new(properties);

        if let Some(layer_tweaker) = &self.base.layer_tweaker {
            layer_tweaker.borrow_mut().update_properties(self.base.evaluated_properties.clone());
        }
    }

    fn has_transition(&self) -> bool {
        self.unevaluated.has_transition()
    }

    fn has_crossfade(&self) -> bool {
        background_crossfade(&self.base.evaluated_properties).t != 1.0
    }

    fn get_solid_background(&self) -> Option<Color> {
        let evaluated = background_layer_properties(&self.base.evaluated_properties);
        if !evaluated.get::<BackgroundPattern>().from.empty() || *evaluated.get::<BackgroundOpacity>() <= 0.0 {
            return None;
        }

        Some(*evaluated.get::<BackgroundColor>() * *evaluated.get::<BackgroundOpacity>())
    }

    fn prepare(&mut self, parameters: &mut LayerPrepareParameters<'_>) {
        let evaluated = background_layer_properties(&self.base.evaluated_properties);
        if !evaluated.get::<BackgroundPattern>().to.empty() {
            // Ensures that the pattern bitmap gets copied to atlas bitmap.
            // Atlas bitmap is uploaded to atlas texture in upload.
            // (conformance)
            add_pattern_if_needed(evaluated.get::<BackgroundPattern>().from.id(), parameters);
            add_pattern_if_needed(evaluated.get::<BackgroundPattern>().to.id(), parameters);
        }
    }

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &dyn Context,
        state: &TransformState,
        update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
        _render_tree: &RenderTree,
        changes: &mut UniqueChangeRequestVec,
    ) {
        let zoom = state.get_integer_zoom();
        let zoom_range = Range::new(0u8, zoom);
        let tile_cover = tile_cover(
            &TileCoverParameters {
                transform_state: state.clone(),
                tile_lod_min_radius: update_parameters.tile_lod_min_radius,
                tile_lod_scale: update_parameters.tile_lod_scale,
                tile_lod_pitch_threshold: update_parameters.tile_lod_pitch_threshold,
                tile_lod_mode: update_parameters.tile_lod_mode,
                requested_zoom: None,
                round_zoom: false,
            },
            zoom,
            zoom_range,
            None,
        );

        // renderTiles is always empty, we use tileCover instead
        if tile_cover.is_empty() {
            self.remove_all_drawables();
            return;
        }

        let evaluated = background_layer_properties(&self.base.evaluated_properties).clone();
        let has_pattern = !evaluated.get::<BackgroundPattern>().to.empty();

        // TODO: If background is solid, we can skip drawables and rely on the
        // clear color
        let draw_passes = self.passes_for(&evaluated);

        // If the result is transparent or missing, just remove any existing
        // drawables and stop
        if draw_passes == RenderPass::None {
            self.remove_all_drawables();
            return;
        }

        if self.base.layer_group.is_none() {
            match context.create_tile_layer_group(
                self.base.layer_index,
                /*initial_capacity=*/ 64,
                self.get_id().to_owned(),
            ) {
                Some(layer_group) => self.set_layer_group(Some(layer_group), changes),
                None => return,
            }
        }
        let layer_group = self.base.layer_group.clone().expect("layer group");

        if self.base.layer_tweaker.is_none() {
            // Background drawables have no source buckets to replace them
            // after a style change. Rebuild them so the new tweaker populates
            // the group's uniform buffers for every drawable.
            self.remove_all_drawables();
            let tweaker: LayerTweakerPtr = Rc::new(RefCell::new(BackgroundLayerTweaker::new(
                self.get_id().to_owned(),
                self.base.evaluated_properties.clone(),
            )));
            self.base.layer_tweaker = Some(tweaker.clone());
            // layerGroup->addLayerTweaker(layerTweaker): the orchestrator
            // keeps the layer groups' tweaker lists (see layer_group.rs).
            changes.push(ChangeRequest::AddLayerTweaker(layer_group.clone(), Rc::downgrade(&tweaker)));
        }

        if self.update_projection_variant(state) {
            self.plain_shader = None;
            self.pattern_shader = None;
        }

        if !has_pattern && self.plain_shader.is_none() {
            self.plain_shader =
                context.get_generic_shader(shaders, BACKGROUND_PLAIN_SHADER_NAME, self.base.projection_variant);
        }
        if has_pattern && self.pattern_shader.is_none() {
            self.pattern_shader =
                context.get_generic_shader(shaders, BACKGROUND_PATTERN_SHADER_NAME, self.base.projection_variant);
        }

        let layer_tweaker = self.base.layer_tweaker.clone().expect("layer tweaker");

        let cur_shader = if has_pattern { self.pattern_shader.clone() } else { self.plain_shader.clone() };
        if cur_shader.is_none() {
            self.remove_all_drawables();
            return;
        }

        let tile_vertices = RenderStaticData::tile_vertices();
        let vertex_count = tile_vertices.len();
        // sizeof(a1): short2
        let vert_size = std::mem::size_of::<[i16; 2]>();
        let mut raw_vertices: Vec<u8> = vec![0; vertex_count * vert_size];
        for (i, vertex) in tile_vertices.iter().enumerate() {
            raw_vertices[i * vert_size..(i + 1) * vert_size].copy_from_slice(bytemuck::bytes_of(vertex));
        }

        let indexes = RenderStaticData::quad_triangle_indices();
        let segs = RenderStaticData::tile_triangle_segments();

        let mut builder: Option<mln_gfx::UniqueDrawableBuilder> = None;

        let mut group = layer_group.borrow_mut();
        let tile_layer_group = group.as_tile_layer_group_mut().expect("background layer group is a TileLayerGroup");

        // Remove drawables for tiles that are no longer in the cover set.
        // (Note that `RenderTiles` is empty, and this layer does not use it)
        tile_layer_group.remove_drawables_if(|drawable| match drawable.get_tile_id() {
            Some(tile_id) => !tile_cover.contains(&tile_id),
            None => false,
        });

        // For each tile in the cover set, add a tile drawable if one doesn't
        // already exist.
        for tile_id in &tile_cover {
            // If we already have drawables for this tile, skip. If a drawable
            // needs to be updated, that's handled in the layer tweaker.
            if tile_layer_group.get_drawable_count_for(draw_passes, tile_id) > 0 {
                continue;
            }

            // We actually need to build things, so set up a builder if we
            // haven't already
            let builder = builder.get_or_insert_with(|| {
                let mut b = context.create_drawable_builder("background".to_owned());
                b.set_render_pass(draw_passes);
                b.set_shader(cur_shader.clone());
                b.set_depth_type(DepthMaskType::ReadWrite);
                b.set_color_mode(&if draw_passes == RenderPass::Translucent {
                    ColorMode::alpha_blended()
                } else {
                    ColorMode::unblended()
                });
                b
            });

            builder.set_vertex_attr_id(ID_BACKGROUND_POS_VERTEX_ATTRIBUTE);
            if self.base.projection_variant == ProjectionVariant::Globe {
                // The background is not depth tested, so it needs no tile
                // border.
                let mesh = raw_globe_tile_mesh(&tile_id.canonical, false, &SubdivisionGranularitySetting::globe().tile);
                builder.set_raw_vertices(mesh.vertices, mesh.vertex_count, AttributeDataType::Short2);
                builder.set_segments(DrawMode::Triangles(), mesh.indices, &mesh.segments);
            } else {
                let vertices_copy = raw_vertices.clone();
                builder.set_raw_vertices(vertices_copy, vertex_count, AttributeDataType::Short2);
                builder.set_segments(DrawMode::Triangles(), indexes.clone(), &segs);
            }
            builder.flush(context);

            for mut drawable in builder.clear_drawables() {
                drawable.set_tile_id(*tile_id);
                drawable.set_layer_tweaker(layer_tweaker_slot(&layer_tweaker));
                tile_layer_group.add_drawable(draw_passes, tile_id, drawable);
            }
        }
    }
}
