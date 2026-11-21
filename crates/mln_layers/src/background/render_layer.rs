use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::ID_PROJECTION_UBO;
use mln_gfx::{AttributeDataType, ColorMode, CullFaceMode, DepthMaskType, DrawMode};
use mln_map_types::tile_cover::{TileCoverParameters, tile_cover};
use mln_render::draw::{
    DrawDepth, DrawFrame, DrawGeometry, DrawIndexes, DrawList, DrawObjects, FrameDraw, KeptDraw, LayerDrawing,
    RenderStaticData, Textures, TileDraws, Uniforms, projection_data, raw_vertices, to_projection_ubo,
};
use mln_render::globe::tile_grid;
use mln_render::layer::{
    DrawContext, EvaluatedLayer, LayerType, LayerValues, NoBucket, NoLayout, NoTileView, RenderPass, UpdateContext,
};
use mln_style_types::TranslateAnchorType;
use mln_tile_id::OverscaledTileID;
use mln_util::Color;
use mln_util::constants::TILE_SIZE_D;
use mln_util::range::Range;

use super::pattern_atlas::PatternAtlas;
use super::programs::{
    ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_IMAGE_TEXTURE, ID_BACKGROUND_POS_VERTEX_ATTRIBUTE,
    ID_BACKGROUND_PROPS_UBO,
};
use super::ubo::{
    BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO,
    BackgroundPropsUBO,
};
use super::{Background, programs};

/// What a background layer keeps besides its drawing.
#[derive(Default)]
pub struct BackgroundState {
    /// The frame's draws as the update makes them, with their uniform blocks
    /// and projections.
    draw_list: DrawList<(BackgroundDrawableUnionUBO, ProjectionUBO)>,
    /// The passes the tiles' draws were built for, and whether with a
    /// pattern.
    draws_built_for: Option<(RenderPass, bool)>,
    /// The tiles that cover the view, which the frame's update picks for
    /// its encode.
    tile_cover: Vec<OverscaledTileID>,
    /// The patterns of `background-pattern`.
    patterns: PatternAtlas,
}

impl BackgroundState {
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
            let canonical = &tile_id.canonical;
            let grid = tile_grid(canonical);
            let count = grid.vertices.len();
            let indices = grid.triangles.into_flattened();
            let segments = vec![SegmentBase::with_lengths(0, 0, count, indices.len())];
            (bytemuck::cast_slice(&grid.vertices).to_vec(), count, indices, segments)
        } else {
            let vertices = RenderStaticData::tile_vertices();
            let count = vertices.len();
            let bytes = bytemuck::cast_slice(&vertices).to_vec();
            (bytes, count, RenderStaticData::quad_triangle_indices(), RenderStaticData::quad_triangle_segments())
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
        frame: &DrawFrame<'_>,
    ) -> (BackgroundDrawableUnionUBO, ProjectionUBO) {
        let tile_id = tile_id.to_unwrapped();
        let projection =
            projection_data(&tile_id, frame, [0.0, 0.0], TranslateAnchorType::Viewport, false, false, None, false);
        let matrix = projection.fallback_matrix.as_mat4().to_cols_array();
        let draw_ubo = if pattern {
            let state = frame.view;
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

impl LayerType for Background {
    type Layout = NoLayout;
    type Bucket = NoBucket;
    type View<'a> = NoTileView;
    type Drawing = TileDraws<KeptDraw>;
    type State = BackgroundState;
    const CROSSFADES: bool = true;

    fn new_layer(_layer: &LayerValues<Background>) -> (BackgroundState, TileDraws<KeptDraw>) {
        Default::default()
    }

    /// Makes the frame's draws of the layer, one of each tile that covers
    /// the view, which it builds for a tile it keeps none of, and writes the
    /// layer's buffers they read.
    fn make_draws(
        state: &mut BackgroundState,
        drawing: &mut TileDraws<KeptDraw>,
        gpu: &mut dyn Gpu,
        cx: &mut DrawContext<'_, '_, Background>,
    ) {
        let (context, frame, variant) = (&mut *cx.context, &cx.frame, cx.projection_variant);
        let BackgroundState { draw_list, draws_built_for, tile_cover, patterns } = state;
        let layer = &*cx.layer.evaluated;
        let evaluated = &layer.paint;
        let has_pattern = !evaluated.background_pattern.faded().to.empty();
        let draw_passes = layer.passes();

        if *draws_built_for != Some((draw_passes, has_pattern)) {
            drawing.clear();
            *draws_built_for = Some((draw_passes, has_pattern));
        }

        let program = if has_pattern { &programs::BACKGROUND_PATTERN } else { &programs::BACKGROUND };
        let shader = context.shader_key(program, Uniforms::default(), variant);

        let faded = evaluated.background_pattern.faded();
        let pattern_positions = if has_pattern {
            patterns.upload(gpu, context);
            let (Some(from), Some(to)) = (patterns.pattern(faded.from.id()), patterns.pattern(faded.to.id())) else {
                // The pattern isn't valid: the layer draws nothing.
                return;
            };
            Some((from, to))
        } else {
            None
        };
        let pattern_texture = patterns.texture().filter(|_| has_pattern);
        let atlas_size = patterns.size();

        let color_mode = context.color_mode(if draw_passes == RenderPass::TRANSLUCENT {
            ColorMode::alpha_blended()
        } else {
            ColorMode::unblended()
        });
        for tile_id in &*tile_cover {
            let Some(entry) = drawing.tiles_mut().entry(tile_id) else {
                continue;
            };
            let cached = entry.value.get_or_insert_with(|| {
                KeptDraw::new(BackgroundState::build_tile(tile_id, shader.clone(), color_mode, variant))
            });
            if pattern_texture.is_some() {
                cached.objects.set_texture(ID_BACKGROUND_IMAGE_TEXTURE, pattern_texture);
            }
            let draw = FrameDraw {
                pass: draw_passes,
                depth: DrawDepth::Layer { sub_layer: 0, mask: DepthMaskType::ReadWrite },
                ..cached.frame_draw(Some(*tile_id))
            };
            draw_list.push(draw, BackgroundState::uniforms(tile_id, has_pattern, frame));
        }

        drawing.finish(draw_list, gpu, context, |writer| {
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
                    texsize: [atlas_size.width as f32, atlas_size.height as f32],
                    pad1: 0.0,
                    pad2: 0.0,
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

    /// None where the layer is transparent, and the opaque pass too where
    /// it is opaque, for the layers below the frame's opaque pass cutoff.
    fn passes(layer: &LayerValues<Background>, evaluated: &EvaluatedLayer<Background>) -> RenderPass {
        let evaluated = &evaluated.paint;
        if *evaluated.background_opacity.uniform() == 0.0 {
            RenderPass::NONE
        } else if !layer.unevaluated.background_pattern.is_undefined()
            || *evaluated.background_opacity.uniform() < 1.0
            || evaluated.background_color.uniform().a < 1.0
        {
            RenderPass::TRANSLUCENT
        } else {
            RenderPass::OPAQUE | RenderPass::TRANSLUCENT
        }
    }

    /// Picks the tiles that cover the view, which the layer draws, having no
    /// source, and packs its patterns, packing again those whose images the
    /// style replaced.
    fn update(
        state: &mut BackgroundState,
        drawing: &mut TileDraws<KeptDraw>,
        cx: &mut UpdateContext<'_, '_, Background>,
    ) {
        let (view, update_parameters) = (cx.view, cx.update_parameters);
        let zoom = view.integer_zoom();
        state.tile_cover = tile_cover(
            &TileCoverParameters {
                view: view.clone(),
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
        drawing.tiles_mut().set_tiles(state.tile_cover.iter().copied());

        let faded = cx.layer.evaluated.paint.background_pattern.faded();
        if !faded.to.empty() {
            // The encode uploads the atlas image with the patterns.
            state.patterns.remove_stale(cx.images);
            state.patterns.add(faded.from.id(), cx.images);
            state.patterns.add(faded.to.id(), cx.images);
        }
    }

    fn solid_background(layer: &EvaluatedLayer<Background>) -> Option<Color> {
        let evaluated = &layer.paint;
        if !evaluated.background_pattern.faded().from.empty() || *evaluated.background_opacity.uniform() <= 0.0 {
            return None;
        }

        Some(*evaluated.background_color.uniform() * *evaluated.background_opacity.uniform())
    }
}
