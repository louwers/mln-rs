use std::sync::Arc;

use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::shader_defines::ID_BACKGROUND_POS_VERTEX_ATTRIBUTE;
use mln_gfx::{AttributeDataType, ColorMode, DepthMaskType, DrawMode, ShaderProgramPtr, ShaderRegistry};
use mln_map_types::TransformState;
use mln_style::layer_type::{ErasedLayerStyle, LayerStyle};
use mln_style::layers::{Background, BackgroundPaintProperties};
use mln_style::{PropertyEvaluationParameters, TransitionParameters};
use mln_util::Color;
use mln_util::range::Range;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use super::{background_crossfade, background_layer_properties};
use crate::bucket::NoBucket;
use crate::drawable_builder::DrawableBuilder;
use crate::gfx::context::Context;
use crate::globe_tile_mesh::raw_globe_tile_mesh;
use crate::layer_group::LayerGroup;
use crate::layer_type::{ErasedEvaluatedLayer, EvaluatedLayer, LayerType};
use crate::layers::background_layer_tweaker::BackgroundLayerTweaker;
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{LayerPrepareParameters, RenderLayer, RenderLayerBase, set_style};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::update_parameters::UpdateParameters;
use crate::util::tile_cover::{TileCoverParameters, tile_cover};

const BACKGROUND_PLAIN_SHADER_NAME: &str = "BackgroundShader";
const BACKGROUND_PATTERN_SHADER_NAME: &str = "BackgroundPatternShader";

/// Adds the image `id` to the pattern atlas if it isn't there yet.
fn add_pattern_if_needed(id: &str, params: &LayerPrepareParameters<'_>) {
    if params.pattern_atlas.borrow().get_pattern(id).is_none()
        && let Some(image) = params.images.get_image(id)
    {
        params.pattern_atlas.borrow_mut().add_pattern(image);
    }
}

impl LayerType for Background {
    type Layout = NoLayout;
    type Bucket = NoBucket;

    fn render_layer(style: Arc<LayerStyle<Background>>) -> Box<dyn RenderLayer> {
        Box::new(RenderBackgroundLayer::new(style))
    }
}

pub(crate) struct RenderBackgroundLayer {
    base: RenderLayerBase,
    style: Arc<LayerStyle<Background>>,
    unevaluated: BackgroundPaintProperties::Unevaluated,
    plain_shader: Option<ShaderProgramPtr>,
    pattern_shader: Option<ShaderProgramPtr>,
}

impl RenderBackgroundLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Background>>) -> Self {
        let unevaluated = impl_.paint.untransitioned();
        let properties: Arc<dyn ErasedEvaluatedLayer> = Arc::new(EvaluatedLayer::<Background>::new(Arc::clone(&impl_)));
        let mut base = RenderLayerBase::new(properties);
        base.style_dependencies = unevaluated.get_dependencies();
        RenderBackgroundLayer { base, style: impl_, unevaluated, plain_shader: None, pattern_shader: None }
    }

    /// The `drawPasses` / `passes` expression shared by `evaluate` and
    /// `update`.
    fn passes_for(&self, evaluated: &BackgroundPaintProperties::PossiblyEvaluated) -> RenderPass {
        if *evaluated.background_opacity.uniform() == 0.0 {
            RenderPass::None
        } else if !self.unevaluated.background_pattern.is_undefined()
            || *evaluated.background_opacity.uniform() < 1.0
            || evaluated.background_color.uniform().a < 1.0
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

    fn restyle(&mut self, parameters: &TransitionParameters, style: &Arc<dyn ErasedLayerStyle>) {
        set_style(&mut self.base, &mut self.style, style);
        self.transition(parameters);
    }

    fn transition(&mut self, parameters: &TransitionParameters) {
        let prior = std::mem::take(&mut self.unevaluated);
        self.unevaluated = self.style.paint.transitioned(parameters, prior);
        self.base.style_dependencies = self.unevaluated.get_dependencies();
    }

    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let previous_evaluated = background_layer_properties(&self.base.evaluated_properties).clone();
        let evaluated = self.unevaluated.evaluate_with_previous(parameters, &previous_evaluated);
        let mut properties = EvaluatedLayer::<Background> {
            crossfade: parameters.get_crossfade_parameters(),
            ..EvaluatedLayer::with_paint(Arc::clone(&self.style), evaluated)
        };

        self.base.passes = self.passes_for(&properties.paint);
        properties.render_passes = self.base.passes.underlying_type();

        self.base.evaluated_properties = Arc::new(properties);

        if let Some(layer_tweaker) = &mut self.base.layer_tweaker {
            layer_tweaker.update_properties(Arc::clone(&self.base.evaluated_properties));
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
        if !evaluated.background_pattern.faded().from.empty() || *evaluated.background_opacity.uniform() <= 0.0 {
            return None;
        }

        Some(*evaluated.background_color.uniform() * *evaluated.background_opacity.uniform())
    }

    fn prepare(&mut self, parameters: &mut LayerPrepareParameters<'_>) {
        let evaluated = background_layer_properties(&self.base.evaluated_properties);
        if !evaluated.background_pattern.faded().to.empty() {
            // Ensures that the pattern bitmap gets copied to atlas bitmap.
            // Atlas bitmap is uploaded to atlas texture in upload.
            // (conformance)
            add_pattern_if_needed(evaluated.background_pattern.faded().from.id(), parameters);
            add_pattern_if_needed(evaluated.background_pattern.faded().to.id(), parameters);
        }
    }

    fn update(
        &mut self,
        shaders: &ShaderRegistry,
        _context: &Context,
        state: &TransformState,
        update_parameters: &Arc<UpdateParameters>,
        _paint_parameters: &PaintParameters<'_>,
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
                max_content_elevation: 0.0,
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
        let has_pattern = !evaluated.background_pattern.faded().to.empty();

        // TODO: If background is solid, we can skip drawables and rely on the
        // clear color
        let draw_passes = self.passes_for(&evaluated);

        // If the result is transparent or missing, just remove any existing
        // drawables and stop
        if draw_passes == RenderPass::None {
            self.remove_all_drawables();
            return;
        }

        let mut layer_group = self.base.layer_group.take().unwrap_or_else(LayerGroup::tiles);
        'draw: {
            if self.base.layer_tweaker.is_none() {
                // Background drawables have no source buckets to replace them
                // after a style change. Rebuild them so the new tweaker populates
                // the group's uniform buffers for every drawable.
                layer_group.clear_drawables();
                self.base.layer_tweaker = Some(Box::new(BackgroundLayerTweaker::new(
                    self.get_id().to_owned(),
                    Arc::clone(&self.base.evaluated_properties),
                )));
            }

            if self.update_projection_variant(state) {
                self.plain_shader = None;
                self.pattern_shader = None;
            }

            if !has_pattern && self.plain_shader.is_none() {
                self.plain_shader = shaders.shader(BACKGROUND_PLAIN_SHADER_NAME, self.base.projection_variant);
            }
            if has_pattern && self.pattern_shader.is_none() {
                self.pattern_shader = shaders.shader(BACKGROUND_PATTERN_SHADER_NAME, self.base.projection_variant);
            }

            let tweaker = self.base.layer_tweaker.as_ref().map(|tweaker| tweaker.base().key);

            let cur_shader = if has_pattern { self.pattern_shader.clone() } else { self.plain_shader.clone() };
            if cur_shader.is_none() {
                layer_group.clear_drawables();
                break 'draw;
            }

            let tile_vertices = RenderStaticData::tile_vertices();
            let vertex_count = tile_vertices.len();
            // sizeof(a1): short2
            let vert_size = size_of::<[i16; 2]>();
            let mut raw_vertices: Vec<u8> = vec![0; vertex_count * vert_size];
            for (i, vertex) in tile_vertices.iter().enumerate() {
                raw_vertices[i * vert_size..(i + 1) * vert_size].copy_from_slice(bytemuck::bytes_of(vertex));
            }

            let indexes = RenderStaticData::quad_triangle_indices();
            let segs = RenderStaticData::tile_triangle_segments();

            let mut builder: Option<crate::drawable_builder::UniqueDrawableBuilder> = None;

            let tile_layer_group = &mut layer_group;

            // Remove drawables for tiles that are no longer in the cover set.
            // (Note that `RenderTiles` is empty, and this layer does not use it)
            tile_layer_group.remove_drawables_if(|drawable| match drawable.tile_id {
                Some(tile_id) => !tile_cover.contains(&tile_id),
                None => false,
            });

            // For each tile in the cover set, add a tile drawable if one doesn't
            // already exist.
            for tile_id in &tile_cover {
                // If we already have drawables for this tile, skip. If a drawable
                // needs to be updated, that's handled in the layer tweaker.
                if tile_layer_group.drawable_count_for(draw_passes, tile_id) > 0 {
                    continue;
                }

                // We actually need to build things, so set up a builder if we
                // haven't already
                let builder = builder.get_or_insert_with(|| {
                    let mut b = DrawableBuilder::boxed();
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
                    let mesh =
                        raw_globe_tile_mesh(&tile_id.canonical, false, SubdivisionGranularitySetting::globe().tile);
                    builder.set_raw_vertices(mesh.vertices, mesh.vertex_count, AttributeDataType::Short2);
                    builder.set_segments(DrawMode::Triangles(), mesh.indices, &mesh.segments);
                } else {
                    let vertices_copy = raw_vertices.clone();
                    builder.set_raw_vertices(vertices_copy, vertex_count, AttributeDataType::Short2);
                    builder.set_segments(DrawMode::Triangles(), indexes.clone(), &segs);
                }
                builder.flush();

                for mut drawable in builder.clear_drawables() {
                    drawable.tile_id = Some(*tile_id);
                    drawable.tweaker = tweaker;
                    tile_layer_group.add_tile_drawable(draw_passes, tile_id, drawable);
                }
            }
        }
        self.base.layer_group = Some(layer_group);
    }
}
