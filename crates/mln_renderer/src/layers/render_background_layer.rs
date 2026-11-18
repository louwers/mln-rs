use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::background_layer_ubo::{
    BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO,
    BackgroundPropsUBO,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_IMAGE_TEXTURE, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE,
    ID_BACKGROUND_PROPS_UBO, ID_PROJECTION_UBO,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode, ShaderProgramPtr};
use mln_style::PropertyEvaluationParameters;
use mln_style::layer_type::LayerStyle;
use mln_style::layers::{Background, BackgroundPaintProperties};
use mln_style_types::TranslateAnchorType;
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::constants::TILE_SIZE_D;
use mln_util::range::Range;
use mln_util::subdivision_granularity::SubdivisionGranularitySetting;

use super::cast_matrix;
use crate::bucket::NoBucket;
use crate::draw::{DrawGeometry, DrawObjects, Textures, raw_vertices};
use crate::gfx::index_vector::IndexVector;
use crate::globe_tile_mesh::raw_globe_tile_mesh;
use crate::layer_drawing::{DrawDepth, DrawList, FrameDraw, KeptDraw, TileDraws};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    Atlases, LayerParts, LayerSource, LayerTiles, LayerUpdateParameters, LayerValues, Parts, PartsMut, RenderLayer,
    RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::tile_matrix::{get_projection_data, to_projection_ubo};
use crate::update_parameters::UpdateParameters;
use crate::util::tile_cover::{TileCoverParameters, tile_cover};

const BACKGROUND_PLAIN_SHADER_NAME: &str = "BackgroundShader";
const BACKGROUND_PATTERN_SHADER_NAME: &str = "BackgroundPatternShader";

/// Adds the image `id` to the pattern atlas if it isn't there yet.
fn add_pattern_if_needed(id: &str, params: &mut LayerUpdateParameters<'_>) {
    if params.atlases.pattern.get_pattern(id).is_none()
        && let Some(image) = params.images.get_image(id)
    {
        params.atlases.pattern.add_pattern(image);
    }
}

impl LayerType for Background {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    const CROSSFADES: bool = true;

    fn render_layer(style: Arc<LayerStyle<Background>>) -> Box<dyn RenderLayer> {
        Box::new(RenderBackgroundLayer::new(style))
    }
}

pub(crate) struct RenderBackgroundLayer {
    base: RenderLayerBase,
    values: LayerValues<Background>,
    drawing: TileDraws<KeptDraw>,
    /// The frame's draws as the update makes them, with their uniform blocks
    /// and projections.
    draw_list: DrawList<(BackgroundDrawableUnionUBO, ProjectionUBO)>,
    plain_shader: Option<ShaderProgramPtr>,
    pattern_shader: Option<ShaderProgramPtr>,
    /// The passes the tiles' draws were built for, and whether with a
    /// pattern.
    draws_built_for: Option<(RenderPass, bool)>,
}

impl RenderBackgroundLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Background>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderBackgroundLayer {
            base,
            values,
            drawing: TileDraws::default(),
            draw_list: DrawList::default(),
            plain_shader: None,
            pattern_shader: None,
            draws_built_for: None,
        }
    }

    /// The GPU objects of the draw of the tile `tile_id`, with `shader`, in
    /// `passes`, in the projection `variant`.
    fn build_tile(
        tile_id: &OverscaledTileID,
        shader: ShaderProgramPtr,
        passes: RenderPass,
        variant: ProjectionVariant,
    ) -> DrawObjects {
        let (vertices, vertex_count, indexes, segments) = if variant == ProjectionVariant::Globe {
            // The background is not depth tested, so it needs no tile border.
            let mesh = raw_globe_tile_mesh(&tile_id.canonical, false, SubdivisionGranularitySetting::globe().tile);
            (mesh.vertices, mesh.vertex_count, mesh.indices, mesh.segments)
        } else {
            let vertices = RenderStaticData::tile_vertices();
            let count = vertices.len();
            let bytes = bytemuck::cast_slice(&vertices).to_vec();
            (bytes, count, RenderStaticData::quad_triangle_indices(), RenderStaticData::tile_triangle_segments())
        };
        let geometry = DrawGeometry {
            vertices: raw_vertices(
                ID_BACKGROUND_POS_VERTEX_ATTRIBUTE,
                vertices,
                vertex_count,
                AttributeDataType::Short2,
            ),
            vertex_count,
            mode: DrawMode::Triangles(),
            indexes: IndexVector::new(indexes),
            segments: segments.iter().map(SegmentBase::copy).collect(),
            instances: None,
        };
        let color_mode =
            if passes == RenderPass::Translucent { ColorMode::alpha_blended() } else { ColorMode::unblended() };
        DrawObjects::new(shader, color_mode, CullFaceMode::disabled(), geometry, Textures::default())
    }

    /// The entries in the layer's storage buffers of the draw of `tile_id`.
    fn uniforms(
        tile_id: &OverscaledTileID,
        pattern: bool,
        parameters: &PaintParameters<'_>,
    ) -> (BackgroundDrawableUnionUBO, ProjectionUBO) {
        let tile_id = tile_id.to_unwrapped();
        let projection = get_projection_data(
            &tile_id,
            parameters,
            [0.0, 0.0],
            TranslateAnchorType::Viewport,
            false,
            false,
            None,
            false,
        );
        let matrix = cast_matrix(&projection.fallback_matrix);
        let draw_ubo = if pattern {
            let state = parameters.state;
            let int_zoom = state.get_integer_zoom();
            let tile_size_at_nearest_zoom =
                (TILE_SIZE_D * state.zoom_scale(f64::from(int_zoom) - f64::from(tile_id.canonical.z))) as i32;
            let pixel_x = (f64::from(tile_size_at_nearest_zoom)
                * (f64::from(tile_id.canonical.x)
                    + f64::from(tile_id.wrap) * state.zoom_scale(f64::from(tile_id.canonical.z))))
                as i32;
            let pixel_y = tile_size_at_nearest_zoom.wrapping_mul(tile_id.canonical.y as i32);
            let pix_to_tile = tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom));
            BackgroundDrawableUnionUBO::from_pattern_drawable(&BackgroundPatternDrawableUBO {
                matrix,
                pixel_coord_upper: [(pixel_x >> 16) as f32, (pixel_y >> 16) as f32],
                pixel_coord_lower: [(pixel_x & 0xFFFF) as f32, (pixel_y & 0xFFFF) as f32],
                tile_units_to_pixels: if pix_to_tile == 0.0 { 0.0 } else { 1.0 / pix_to_tile },
                pad1: 0.0,
                pad2: 0.0,
                pad3: 0.0,
            })
        } else {
            BackgroundDrawableUnionUBO::from_drawable(&BackgroundDrawableUBO { matrix })
        };
        (draw_ubo, to_projection_ubo(&projection))
    }

    /// The `drawPasses` / `passes` expression shared by `evaluate` and
    /// `update`.
    fn passes_for(&self, evaluated: &BackgroundPaintProperties::PossiblyEvaluated) -> RenderPass {
        if *evaluated.background_opacity.uniform() == 0.0 {
            RenderPass::None
        } else if !self.values.unevaluated.background_pattern.is_undefined()
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

impl TypedLayer for RenderBackgroundLayer {
    type Type = Background;
    type Drawing = TileDraws<KeptDraw>;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    /// Makes the frame's draws of the layer, one of each tile that covers
    /// the view, which it builds for a tile it keeps none of, and writes the
    /// layer's buffers they read.
    fn update_draws(
        &mut self,
        paint_parameters: &mut PaintParameters<'_>,
        atlases: &Atlases,
        update_parameters: &UpdateParameters,
        _source: Option<LayerSource<'_, Background>>,
    ) {
        let (static_data, state) = (paint_parameters.static_data, paint_parameters.state);
        let shaders = &static_data.shaders;
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

        // The layer has no render tiles: it draws the tiles that cover the
        // view.
        if tile_cover.is_empty() {
            self.remove_all_draws();
            return;
        }

        let evaluated_properties = Arc::clone(&self.values.evaluated);
        let evaluated = &evaluated_properties.paint;
        let has_pattern = !evaluated.background_pattern.faded().to.empty();

        // TODO: If background is solid, we can skip the draws and rely on the
        // clear color
        let draw_passes = self.passes_for(evaluated);

        // If the result is transparent or missing, just remove any existing
        // draws and stop
        if draw_passes == RenderPass::None {
            self.remove_all_draws();
            return;
        }
        self.drawing.frame_mut();

        if self.update_projection_variant(state) {
            self.plain_shader = None;
            self.pattern_shader = None;
        }
        if self.draws_built_for != Some((draw_passes, has_pattern)) {
            self.remove_all_draws();
            self.draws_built_for = Some((draw_passes, has_pattern));
        }

        if !has_pattern && self.plain_shader.is_none() {
            self.plain_shader = shaders.shader(BACKGROUND_PLAIN_SHADER_NAME, self.base.projection_variant);
        }
        if has_pattern && self.pattern_shader.is_none() {
            self.pattern_shader = shaders.shader(BACKGROUND_PATTERN_SHADER_NAME, self.base.projection_variant);
        }
        let Some(shader) = (if has_pattern { self.pattern_shader.clone() } else { self.plain_shader.clone() }) else {
            self.remove_all_draws();
            return;
        };

        self.drawing.tiles_mut().set_tiles(tile_cover.iter().copied());

        let pattern_atlas = &atlases.pattern;
        let faded = evaluated.background_pattern.faded();
        let pattern_positions = if has_pattern {
            let (Some(from), Some(to)) =
                (pattern_atlas.get_pattern(faded.from.id()), pattern_atlas.get_pattern(faded.to.id()))
            else {
                // The pattern isn't valid: the layer draws nothing.
                self.drawing.frame_mut().clear();
                return;
            };
            Some((from, to))
        } else {
            None
        };
        let pattern_texture = pattern_atlas.texture().filter(|_| has_pattern);
        if let Some(texture) = pattern_texture.and_then(|texture| paint_parameters.context.texture_mut(texture)) {
            texture.set_sampler(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        }

        let variant = self.base.projection_variant;
        for tile_id in &tile_cover {
            let Some(entry) = self.drawing.tiles_mut().entry(tile_id) else {
                continue;
            };
            let cached = entry.value.get_or_insert_with(|| {
                KeptDraw::new(Self::build_tile(tile_id, Rc::clone(&shader), draw_passes, variant))
            });
            if let Some(texture) = pattern_texture {
                cached.objects.set_texture(ID_BACKGROUND_IMAGE_TEXTURE, Some(texture));
            }
            let draw = FrameDraw {
                pass: draw_passes,
                depth: DrawDepth::Layer { sub_layer: 0, mask: DepthMaskType::ReadWrite },
                ..cached.frame_draw(Some(*tile_id))
            };
            self.draw_list.push(draw, Self::uniforms(tile_id, has_pattern, paint_parameters));
        }

        let frame = self.drawing.frame_mut();
        self.draw_list.finish(frame, paint_parameters.context, |writer| {
            let opacity = *evaluated.background_opacity.uniform();
            if let Some((from, to)) = pattern_positions {
                let crossfade = &evaluated_properties.crossfade;
                let props = BackgroundPatternPropsUBO {
                    pattern_tl_a: from.tl().map(f32::from),
                    pattern_br_a: from.br().map(f32::from),
                    pattern_tl_b: to.tl().map(f32::from),
                    pattern_br_b: to.br().map(f32::from),
                    pattern_size_a: from.display_size(),
                    pattern_size_b: to.display_size(),
                    scale_a: crossfade.from_scale,
                    scale_b: crossfade.to_scale,
                    mix: crossfade.t,
                    opacity,
                };
                writer.uniform(ID_BACKGROUND_PROPS_UBO, &props);
            } else {
                let props = BackgroundPropsUBO {
                    color: evaluated.background_color.uniform().to_float_array(),
                    opacity,
                    pad1: 0.0,
                    pad2: 0.0,
                    pad3: 0.0,
                };
                writer.uniform(ID_BACKGROUND_PROPS_UBO, &props);
            }
            writer.storage(ID_BACKGROUND_DRAWABLE_UBO, |(draw_ubo, _)| *draw_ubo);
            writer.storage(ID_PROJECTION_UBO, |(_, projection)| *projection);
        });
    }

    /// Adds the layer's pattern to the pattern atlas. The layer draws
    /// the tiles that cover the view, not its source's.
    fn update(&mut self, parameters: &mut LayerUpdateParameters<'_>, _tiles: Option<LayerTiles<'_, Background>>) {
        let evaluated = &self.values.evaluated.paint;
        if !evaluated.background_pattern.faded().to.empty() {
            // The encode uploads the atlas image with the patterns.
            add_pattern_if_needed(evaluated.background_pattern.faded().from.id(), parameters);
            add_pattern_if_needed(evaluated.background_pattern.faded().to.id(), parameters);
        }
    }
}

impl RenderLayer for RenderBackgroundLayer {
    fn evaluate(&mut self, parameters: &PropertyEvaluationParameters) {
        let previous_evaluated = self.values.evaluated.paint.clone();
        let evaluated = self.values.unevaluated.evaluate_with_previous(parameters, &previous_evaluated);
        let properties = EvaluatedLayer::<Background> {
            crossfade: parameters.get_crossfade_parameters(),
            ..EvaluatedLayer::with_paint(Arc::clone(&self.values.style), evaluated)
        };

        self.base.passes = self.passes_for(&properties.paint);
        self.values.set_evaluated(&mut self.base, properties);
    }

    fn solid_background(&self) -> Option<Color> {
        let evaluated = &self.values.evaluated.paint;
        if !evaluated.background_pattern.faded().from.empty() || *evaluated.background_opacity.uniform() <= 0.0 {
            return None;
        }

        Some(*evaluated.background_color.uniform() * *evaluated.background_opacity.uniform())
    }
}
