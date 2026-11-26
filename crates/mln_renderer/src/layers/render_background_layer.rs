use std::sync::Arc;

use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::bindings::PROJECTION_ID;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode};
use mln_style::color::Color;
use mln_style::constants::TILE_SIZE_D;
use mln_style::layer::LayerStyle;
use mln_style::layers::Background;
use mln_style::range::Range;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::types::TranslateAnchorType;
use mln_tile_id::OverscaledTileID;

use super::cast_matrix;
use crate::bucket::NoBucket;
use crate::draw::{DrawGeometry, DrawObjects, Textures, raw_vertices};
use crate::gfx::indexes::DrawIndexes;
use crate::gfx::segment::SegmentBase;
use crate::gfx::vertex_attribute::Uniforms;
use crate::globe_tile_mesh::raw_globe_tile_mesh;
use crate::layer_drawing::{DrawDepth, DrawList, DrawObjectsMut, FrameDraw, KeptDraw, TileDraws, TileObjects};
use crate::layer_type::{EvaluatedLayer, LayerType};
use crate::layers::background::programs;
use crate::layers::background::ubo::{
    BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO,
    BackgroundPropsUBO,
};
use crate::layout::layout::NoLayout;
use crate::paint_parameters::PaintParameters;
use crate::render_layer::{
    Atlases, LayerUpdateParameters, LayerValues, Parts, PartsMut, RenderLayer, RenderLayerBase, TypedLayer,
};
use crate::render_pass::RenderPass;
use crate::render_static_data::RenderStaticData;
use crate::render_tile::NoTileBucket;
use crate::tile_matrix::{projection_data, to_projection_ubo};
use crate::util::tile_cover::{TileCoverParameters, tile_cover};

/// The ids of the buffers that the draws bind.
const DRAWABLE_ID: usize = programs::BACKGROUND.buffer_id("drawableVector");
const PROPS_ID: usize = programs::BACKGROUND.buffer_id("props");

/// Adds the image `id` to the pattern atlas if it isn't there yet.
fn add_pattern_if_needed(id: &str, params: &mut LayerUpdateParameters<'_>) {
    if params.atlases.pattern.pattern(id).is_none()
        && let Some(image) = params.images.image(id)
    {
        params.atlases.pattern.add_pattern(image);
    }
}

impl LayerType for Background {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    type TileBucket<'a> = NoTileBucket;
    const CROSSFADES: bool = true;

    fn render_layer(style: Arc<LayerStyle<Background>>) -> Box<dyn RenderLayer> {
        Box::new(RenderBackgroundLayer::new(style))
    }
}

/// What the layer keeps of a tile: its draw, and the passes it was built
/// for and whether with a pattern.
pub(crate) struct BackgroundTile {
    draw: KeptDraw,
    built_for: (RenderPass, bool),
}

impl TileObjects for BackgroundTile {
    type Part = ();

    fn objects(&mut self, (): ()) -> Option<DrawObjectsMut<'_>> {
        self.draw.objects(())
    }

    fn drop_bindings(&mut self) {
        self.draw.drop_bindings();
    }
}

pub(crate) struct RenderBackgroundLayer {
    base: RenderLayerBase,
    values: LayerValues<Background>,
    drawing: TileDraws<BackgroundTile>,
    /// The frame's draws as the update makes them, with their uniform blocks
    /// and projections.
    draw_list: DrawList<(BackgroundDrawableUnionUBO, ProjectionUBO)>,
}

impl RenderBackgroundLayer {
    pub(crate) fn new(impl_: Arc<LayerStyle<Background>>) -> Self {
        let (base, values) = LayerValues::new(impl_);
        RenderBackgroundLayer { base, values, drawing: TileDraws::default(), draw_list: DrawList::default() }
    }

    /// The GPU objects of the draw of the tile `tile_id`, with `shader` and
    /// `color_mode`, in the projection `variant`.
    fn build_tile(
        tile_id: &OverscaledTileID,
        shader: ShaderKey,
        color_mode: ColorMode,
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
            vertices: raw_vertices("pos", vertices, vertex_count, AttributeDataType::Short2),
            vertex_count,
            mode: DrawMode::Triangles(),
            indexes: DrawIndexes::new(indexes),
            segments: segments.iter().map(SegmentBase::copy).collect(),
            instances: None,
        };
        DrawObjects::new(shader, color_mode, CullFaceMode::disabled(), geometry, Textures::default())
    }

    /// The entries in the layer's storage buffers of the draw of `tile_id`.
    fn uniforms(
        tile_id: &OverscaledTileID,
        pattern: bool,
        parameters: &PaintParameters<'_>,
    ) -> (BackgroundDrawableUnionUBO, ProjectionUBO) {
        let tile_id = tile_id.to_unwrapped();
        let projection =
            projection_data(&tile_id, parameters, [0.0, 0.0], TranslateAnchorType::Viewport, false, false, None, false);
        let matrix = cast_matrix(&projection.fallback_matrix);
        let draw_ubo = if pattern {
            let state = parameters.view;
            let int_zoom = state.integer_zoom();
            let tile_size_at_nearest_zoom =
                (TILE_SIZE_D * mln_map_types::zoom_scale(f64::from(int_zoom) - f64::from(tile_id.canonical.z))) as i32;
            let pixel_x = (f64::from(tile_size_at_nearest_zoom)
                * (f64::from(tile_id.canonical.x)
                    + f64::from(tile_id.wrap) * mln_map_types::zoom_scale(f64::from(tile_id.canonical.z))))
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
}

impl TypedLayer for RenderBackgroundLayer {
    type Type = Background;
    type Drawing = TileDraws<BackgroundTile>;
    const DRAPES: bool = true;

    fn parts(&self) -> Parts<'_, Self> {
        Parts { base: &self.base, values: &self.values, drawing: &self.drawing }
    }

    fn parts_mut(&mut self) -> PartsMut<'_, Self> {
        PartsMut { base: &mut self.base, values: &mut self.values, drawing: &mut self.drawing }
    }

    /// Makes the frame's draws of the layer, one of each tile it keeps: the
    /// tiles that cover the view, or a drape's tile. It builds the draw of a
    /// tile it keeps none of or whose draw was built for other passes or
    /// patterns, and writes the layer's buffers the draws read.
    fn update_draws_without_source(
        &mut self,
        gpu: &mut dyn Gpu,
        paint_parameters: &mut PaintParameters<'_>,
        atlases: &Atlases,
    ) {
        let RenderBackgroundLayer { base, values, drawing, draw_list } = self;
        let layer = &*values.evaluated;
        let evaluated = &layer.paint;
        let has_pattern = !evaluated.background_pattern.faded().to.empty();
        let draw_passes = RenderPass(layer.render_passes);
        let built_for = (draw_passes, has_pattern);

        let program = if has_pattern { &programs::BACKGROUND_PATTERN } else { &programs::BACKGROUND };
        let shader = paint_parameters.context.shader_key(program, Uniforms::default(), base.projection_variant);

        let pattern_atlas = &atlases.pattern;
        let faded = evaluated.background_pattern.faded();
        let pattern_positions = if has_pattern {
            let (Some(from), Some(to)) = (pattern_atlas.pattern(faded.from.id()), pattern_atlas.pattern(faded.to.id()))
            else {
                // The pattern isn't valid: the layer draws nothing.
                return;
            };
            Some((from, to))
        } else {
            None
        };
        let pattern_texture = pattern_atlas.texture().filter(|_| has_pattern);
        if let Some(mut texture) = pattern_texture.and_then(|texture| paint_parameters.context.texture_mut(texture)) {
            texture.set_sampler(SamplerState {
                filter: TextureFilterType::Linear,
                wrap_u: TextureWrapType::Clamp,
                wrap_v: TextureWrapType::Clamp,
                ..Default::default()
            });
        }

        let variant = base.projection_variant;
        let color_mode =
            if draw_passes == RenderPass::Translucent { ColorMode::alpha_blended() } else { ColorMode::unblended() };
        for (tile_id, entry) in drawing.tiles_mut().entries() {
            if entry.value.as_ref().is_some_and(|cached| cached.built_for != built_for) {
                *entry.value = None;
            }
            let cached = entry.value.get_or_insert_with(|| BackgroundTile {
                draw: KeptDraw::new(Self::build_tile(&tile_id, shader.clone(), color_mode, variant)),
                built_for,
            });
            if pattern_texture.is_some() {
                cached
                    .draw
                    .objects
                    .set_texture(const { programs::BACKGROUND_PATTERN.texture_slot("image") }, pattern_texture);
            }
            let draw = FrameDraw {
                pass: draw_passes,
                depth: DrawDepth::Layer { sub_layer: 0, mask: DepthMaskType::ReadWrite },
                ..cached.draw.frame_draw(Some(tile_id))
            };
            draw_list.push(draw, Self::uniforms(&tile_id, has_pattern, paint_parameters));
        }

        drawing.finish(draw_list, gpu, paint_parameters.context, |writer| {
            let opacity = *evaluated.background_opacity.uniform();
            if let Some((from, to)) = pattern_positions {
                let crossfade = &layer.crossfade;
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
                writer.uniform(PROPS_ID, &props);
            } else {
                let props = BackgroundPropsUBO {
                    color: evaluated.background_color.uniform().to_float_array(),
                    opacity,
                    pad1: 0.0,
                    pad2: 0.0,
                    pad3: 0.0,
                };
                writer.uniform(PROPS_ID, &props);
            }
            writer.storage(DRAWABLE_ID, |(draw_ubo, _)| *draw_ubo);
            writer.storage(PROJECTION_ID, |(_, projection)| *projection);
        });
    }

    /// None where the layer is transparent, and the opaque pass too where
    /// it is opaque, for the layers below the frame's opaque pass cutoff.
    fn passes(&self, evaluated: &EvaluatedLayer<Background>) -> RenderPass {
        let evaluated = &evaluated.paint;
        if *evaluated.background_opacity.uniform() == 0.0 {
            RenderPass::None
        } else if !self.values.unevaluated.background_pattern.is_undefined()
            || *evaluated.background_opacity.uniform() < 1.0
            || evaluated.background_color.uniform().a < 1.0
        {
            RenderPass::Translucent
        } else {
            RenderPass::Opaque | RenderPass::Translucent
        }
    }

    /// Picks the tiles that cover the view, which the layer draws, having no
    /// source, and adds its pattern to the pattern atlas.
    fn update_without_source(&mut self, parameters: &mut LayerUpdateParameters<'_>) {
        let (state, update_parameters) = (parameters.view, parameters.update_parameters);
        let zoom = state.integer_zoom();
        let tiles = tile_cover(
            &TileCoverParameters {
                view: state.clone(),
                tile_lod_min_radius: update_parameters.tile_lod_min_radius,
                tile_lod_scale: update_parameters.tile_lod_scale,
                tile_lod_pitch_threshold: update_parameters.tile_lod_pitch_threshold,
                tile_lod_mode: update_parameters.tile_lod_mode,
                requested_zoom: None,
                round_zoom: false,
                max_content_elevation: 0.0,
            },
            zoom,
            Range::new(0u8, zoom),
            None,
        );
        self.drawing.tiles_mut().set_tiles(tiles);

        let evaluated = &self.values.evaluated.paint;
        if !evaluated.background_pattern.faded().to.empty() {
            // The encode uploads the atlas image with the patterns.
            add_pattern_if_needed(evaluated.background_pattern.faded().from.id(), parameters);
            add_pattern_if_needed(evaluated.background_pattern.faded().to.id(), parameters);
        }
    }
}

impl RenderLayer for RenderBackgroundLayer {
    fn solid_background(&self) -> Option<Color> {
        let evaluated = &self.values.evaluated.paint;
        if !evaluated.background_pattern.faded().from.empty() || *evaluated.background_opacity.uniform() <= 0.0 {
            return None;
        }

        Some(*evaluated.background_color.uniform() * *evaluated.background_opacity.uniform())
    }
}
