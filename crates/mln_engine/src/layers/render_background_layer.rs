use std::sync::Arc;

use mln_gfx::gpu::ShaderKey;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode};
use mln_style::constants::TILE_SIZE_D;
use mln_style::layer::LayerStyle;
use mln_style::layers::Background;
use mln_style::range::Range;
use mln_style::subdivision_granularity::SubdivisionGranularitySetting;
use mln_style::types::TranslateAnchorType;

use super::cast_matrix;
use crate::draw::context::Context;
use crate::draw::draws::{Depth, Draw, Draws, LayerFrame};
use crate::draw::indexes::DrawIndexes;
use crate::draw::objects::{DrawGeometry, Mesh, Textures, raw_vertices};
use crate::draw::resources::Resources;
use crate::draw::segment::SegmentBase;
use crate::draw::vertex_attribute::Uniforms;
use crate::layer::layer_type::LayerType;
use crate::layer::render_layer::{
    EvaluateCx, Evaluation, Hosted, LayerValues, OnTerrain, PrepareCx, RenderLayer, TypedLayer, UpdateCx,
};
use crate::layer::tile_input::NoTiles;
use crate::layers::background::pattern_atlas::PatternAtlas;
use crate::layers::background::programs;
use crate::layers::background::ubo::{
    BackgroundDrawableUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO, BackgroundPropsUBO,
};
use crate::render::globe::globe_tile_mesh::raw_globe_tile_mesh;
use crate::render::pass::RenderPass;
use crate::render::static_data::RenderStaticData;
use crate::render::tile::FrameTile;
use crate::render::tile_matrix::{projection_data, to_projection_ubo};
use crate::tile::OverscaledTileID;
use crate::tile_store::tile_cover::{TileCoverParameters, tile_cover};

impl LayerType for Background {
    type Input = NoTiles;
    const CROSSFADES: bool = true;

    fn render_layer(style: Arc<LayerStyle<Background>>) -> Box<dyn RenderLayer> {
        Hosted::<RenderBackgroundLayer>::boxed(style)
    }
}

/// What the layer keeps of a tile: its mesh, and the passes it was built
/// for and whether with a pattern.
pub(crate) struct BackgroundTile {
    mesh: Mesh,
    built_for: (RenderPass, bool),
}

/// The block of a tile's draw, of the program with or without a pattern.
enum DrawableBlock {
    Plain(BackgroundDrawableUBO),
    Pattern(BackgroundPatternDrawableUBO),
}

pub(crate) struct RenderBackgroundLayer {
    /// The patterns of `background-pattern`.
    patterns: PatternAtlas,
}

impl RenderBackgroundLayer {
    /// The passes a tile's draw draws in with the layer's `values`, and
    /// whether it draws a pattern.
    fn built_for(values: &LayerValues<Background>) -> (RenderPass, bool) {
        let layer = &*values.evaluated;
        let has_pattern = !layer.paint.background_pattern.faded().to.empty();
        (RenderPass(layer.render_passes), has_pattern)
    }

    /// The mesh of the draw of the tile `tile_id`, with `shader` and
    /// `color_mode`, in the projection `variant`.
    fn build_tile(
        context: &mut Context,
        tile_id: &OverscaledTileID,
        shader: ShaderKey,
        color_mode: ColorMode,
        variant: ProjectionVariant,
    ) -> Mesh {
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
        Mesh::new(context, shader, color_mode, CullFaceMode::disabled(), geometry, Textures::default())
    }

    /// The block of the draw of `tile_id`, and its projection.
    fn uniforms(tile_id: &OverscaledTileID, pattern: bool, frame: &LayerFrame<'_>) -> (DrawableBlock, ProjectionUBO) {
        let tile_id = tile_id.to_unwrapped();
        let projection =
            projection_data(&tile_id, frame, [0.0, 0.0], TranslateAnchorType::Viewport, false, false, None, false);
        let matrix = cast_matrix(&projection.fallback_matrix);
        let draw_ubo = if pattern {
            let state = frame.view;
            let int_zoom = state.integer_zoom();
            let tile_size_at_nearest_zoom =
                (TILE_SIZE_D * crate::camera::zoom_scale(f64::from(int_zoom) - f64::from(tile_id.canonical.z))) as i32;
            let pixel_x = (f64::from(tile_size_at_nearest_zoom)
                * (f64::from(tile_id.canonical.x)
                    + f64::from(tile_id.wrap) * crate::camera::zoom_scale(f64::from(tile_id.canonical.z))))
                as i32;
            let pixel_y = tile_size_at_nearest_zoom.wrapping_mul(tile_id.canonical.y as i32);
            let pix_to_tile = tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom));
            DrawableBlock::Pattern(BackgroundPatternDrawableUBO {
                matrix,
                pixel_coord_upper: [(pixel_x >> 16) as f32, (pixel_y >> 16) as f32],
                pixel_coord_lower: [(pixel_x & 0xFFFF) as f32, (pixel_y & 0xFFFF) as f32],
                tile_units_to_pixels: if pix_to_tile == 0.0 { 0.0 } else { 1.0 / pix_to_tile },
                pad1: 0.0,
                pad2: 0.0,
                pad3: 0.0,
            })
        } else {
            DrawableBlock::Plain(BackgroundDrawableUBO { matrix })
        };
        (draw_ubo, to_projection_ubo(&projection))
    }
}

impl TypedLayer for RenderBackgroundLayer {
    type Type = Background;
    type Tile = BackgroundTile;
    const TERRAIN: OnTerrain = OnTerrain::Drapes;
    const PICKS_TILES: bool = true;

    fn new(_values: &LayerValues<Background>) -> Self {
        RenderBackgroundLayer { patterns: PatternAtlas::new() }
    }

    /// Keeps a tile's draw built for the passes and pattern the layer draws
    /// with `values`.
    fn keeps(&self, values: &LayerValues<Background>, tile: &BackgroundTile) -> bool {
        tile.built_for == Self::built_for(values)
    }

    /// Pushes the frame's draws of the layer, one of each tile it picked:
    /// the tiles that cover the view, or a drape's tile. It builds the mesh
    /// of a tile it keeps none of.
    fn prepare(&mut self, cx: &mut PrepareCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Background>], out: &mut Draws) {
        let RenderBackgroundLayer { patterns } = self;
        let PrepareCx { base, values, tiles, picked, res, frame } = cx;
        let Resources { gpu, context } = res;
        let layer = &*values.evaluated;
        let evaluated = &layer.paint;
        let built_for = Self::built_for(values);
        let (draw_passes, has_pattern) = built_for;

        let program = if has_pattern { &programs::BACKGROUND_PATTERN } else { &programs::BACKGROUND };
        let shader = context.shader_key(program, Uniforms::default(), base.projection_variant);

        let faded = evaluated.background_pattern.faded();
        let pattern_positions = if has_pattern {
            let (Some(from), Some(to)) = (patterns.pattern(faded.from.id()), patterns.pattern(faded.to.id())) else {
                // The pattern isn't valid: the layer draws nothing.
                return;
            };
            Some((from, to))
        } else {
            None
        };
        if has_pattern {
            patterns.upload(*gpu, context);
        }
        let pattern_texture = patterns.texture().filter(|_| has_pattern);
        if let Some(mut texture) = pattern_texture.and_then(|texture| context.texture_mut(texture)) {
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
        for &tile_id in *picked {
            let Some(entry) = tiles.entry(&tile_id) else { continue };
            let cached = entry.value.get_or_insert_with(|| BackgroundTile {
                mesh: Self::build_tile(context, &tile_id, shader.clone(), color_mode, variant),
                built_for,
            });
            if pattern_texture.is_some() {
                cached.mesh.set_texture(
                    context,
                    const { programs::BACKGROUND_PATTERN.texture_slot("image") },
                    pattern_texture,
                );
            }
            let draw = Draw {
                order: cached.mesh.seq(),
                opaque: draw_passes & RenderPass::Opaque != RenderPass::None,
                depth: Depth::Layer { sub_layer: 0, mask: DepthMaskType::ReadWrite },
                ..Draw::DEFAULT
            };
            let group = out.main();
            match Self::uniforms(&tile_id, has_pattern, frame) {
                (DrawableBlock::Plain(block), projection) => {
                    group.push_at(&cached.mesh, tile_id, &projection, draw, block);
                }
                (DrawableBlock::Pattern(block), projection) => {
                    group.push_at(&cached.mesh, tile_id, &projection, draw, block);
                }
            }
        }

        let texsize = patterns.pixel_size();
        let opacity = *evaluated.background_opacity.uniform();
        let group = out.main();
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
                texsize: [texsize.width as f32, texsize.height as f32],
                pad1: 0.0,
                pad2: 0.0,
            };
            group.block(&props);
        } else {
            let props = BackgroundPropsUBO {
                color: evaluated.background_color.uniform().to_float_array(),
                opacity,
                pad1: 0.0,
                pad2: 0.0,
                pad3: 0.0,
            };
            group.block(&props);
        }
    }

    /// Draws nothing where the layer is transparent, and in the opaque pass
    /// too where it is opaque, for the layers below the frame's opaque pass
    /// cutoff. Without a pattern and where it shows, it is one solid color.
    fn evaluate(&mut self, cx: EvaluateCx<'_, Self>) -> Evaluation {
        let evaluated = &cx.evaluated.paint;
        let opacity = *evaluated.background_opacity.uniform();
        let color = *evaluated.background_color.uniform();
        let evaluation = if opacity == 0.0 {
            Evaluation::NONE
        } else if !cx.values.unevaluated.background_pattern.is_undefined() || opacity < 1.0 || color.a < 1.0 {
            Evaluation::TRANSLUCENT
        } else {
            Evaluation::OPAQUE
        };
        let pattern = !evaluated.background_pattern.faded().from.empty();
        let solid = (!pattern && opacity > 0.0).then(|| color * opacity);
        Evaluation { solid, ..evaluation }
    }

    /// Picks the tiles that cover the view, which the layer draws, having no
    /// source, and packs its patterns, again those whose images the style
    /// replaced.
    fn update(&mut self, mut cx: UpdateCx<'_, '_, Self>, _tiles: &[FrameTile<'_, Background>]) {
        let (state, update_parameters, images) =
            (cx.parameters.view, cx.parameters.update_parameters, cx.parameters.images);
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
        cx.pick_tiles(tiles);

        let evaluated = &cx.values.evaluated.paint;
        let faded = evaluated.background_pattern.faded();
        if !faded.to.empty() {
            // The encode uploads the atlas image with the patterns.
            self.patterns.remove_stale(images);
            self.patterns.add(faded.from.id(), images);
            self.patterns.add(faded.to.id(), images);
        }
    }
}
