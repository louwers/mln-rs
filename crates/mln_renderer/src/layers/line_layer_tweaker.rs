use std::sync::Arc;

use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::line_layer_ubo::{
    LineDrawableUBO, LineDrawableUnionUBO, LineEvaluatedPropsUBO, LineGradientDrawableUBO, LinePatternDrawableUBO,
    LinePatternTilePropsUBO, LineSDFDrawableUBO, LineSDFTilePropsUBO, LineTilePropsUnionUBO,
};
use mln_gfx::shaders::shader_defines::{
    ID_LINE_DASH_TEXTURE, ID_LINE_DRAWABLE_UBO, ID_LINE_EVALUATED_PROPS_UBO, ID_LINE_IMAGE_TEXTURE,
    ID_LINE_TILE_PROPS_UBO,
};
use mln_style::layers::{Line, LinePaintProperties};
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use super::{DrawableRenderTile, cast_matrix, slot_bucket};
use crate::buckets::line_bucket::LineBucket;
use crate::gfx::uniform_buffer::UniformBufferPtr;
use crate::layer_group::LayerGroup;
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_projection_data, to_projection_ubo};
use crate::layer_type::{ErasedEvaluatedLayer, evaluated};
use crate::line_atlas::LinePatternCap;
use crate::paint_parameters::PaintParameters;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum LineType {
    Simple,
    Gradient,
    Pattern,
    Sdf,
    GradientSdf,
}

impl LineType {
    #[must_use]
    pub(crate) fn from_type(type_: usize) -> Option<LineType> {
        match type_ {
            0 => Some(LineType::Simple),
            1 => Some(LineType::Gradient),
            2 => Some(LineType::Pattern),
            3 => Some(LineType::Sdf),
            4 => Some(LineType::GradientSdf),
            _ => None,
        }
    }
}

/// A line drawable's data: the line cap its dash pattern was built for.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LineDrawableData {
    pub(crate) line_pattern_cap: LinePatternCap,
}

/// Where a dash drawable samples its patterns. Where the dash array differs
/// by feature, the shaders divide the scales' x and the gamma by each
/// pattern's length.
struct SdfPatterns {
    patternscale_a: [f32; 2],
    patternscale_b: [f32; 2],
    tex_y_a: f32,
    tex_y_b: f32,
    atlas_height: f32,
    sdfgamma: f32,
}

pub(crate) struct LineLayerTweaker {
    base: LayerTweakerBase,
    evaluated_props_uniform_buffer: Option<UniformBufferPtr>,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
    tile_props_uniform_buffer: Option<UniformBufferPtr>,
}

impl LineLayerTweaker {
    pub(crate) fn new(id: String, properties: Arc<dyn ErasedEvaluatedLayer>) -> Self {
        LineLayerTweaker {
            base: LayerTweakerBase::new(id, Some(properties)),
            evaluated_props_uniform_buffer: None,
            drawable_uniform_buffer: None,
            tile_props_uniform_buffer: None,
        }
    }
}

fn cast_tlbr(a: [u16; 4]) -> [f32; 4] {
    [f32::from(a[0]), f32::from(a[1]), f32::from(a[2]), f32::from(a[3])]
}

impl LayerTweaker for LineLayerTweaker {
    fn base(&self) -> &LayerTweakerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut LayerTweakerBase {
        &mut self.base
    }

    fn execute(&mut self, layer_group: &mut LayerGroup, parameters: &PaintParameters<'_>) {
        if layer_group.empty() {
            return;
        }

        let context = parameters.context;
        let Some(properties) = self.base.evaluated_properties.clone() else {
            return;
        };
        let layer = evaluated::<Line>(&properties);
        let evaluated = &layer.paint;
        let crossfade = layer.crossfade;

        let line_pattern_value = evaluated.line_pattern.faded();

        let zoom = parameters.state.get_zoom() as f32;
        let int_zoom = parameters.state.get_integer_zoom();
        let state = parameters.state;
        let pixel_scale = state.get_projection().pixel_scale(state) as f32;

        if self.evaluated_props_uniform_buffer.is_none() || self.base.properties_updated {
            let props_ubo = LineEvaluatedPropsUBO {
                color: evaluated.line_color.constant_or(&LinePaintProperties::Defaults::line_color()).to_float_array(),
                blur: evaluated.line_blur.constant_or(&LinePaintProperties::Defaults::line_blur()),
                opacity: evaluated.line_opacity.constant_or(&LinePaintProperties::Defaults::line_opacity()),
                gapwidth: evaluated.line_gap_width.constant_or(&LinePaintProperties::Defaults::line_gap_width()),
                offset: evaluated.line_offset.constant_or(&LinePaintProperties::Defaults::line_offset()),
                width: evaluated.line_width.constant_or(&LinePaintProperties::Defaults::line_width()),
                floorwidth: evaluated.line_floor_width.constant_or(&LinePaintProperties::Defaults::line_floor_width()),
                pad0: 0.0,
                pad1: 0.0,
            };
            context.write_uniform_buffer_pod(&mut self.evaluated_props_uniform_buffer, &props_ubo);
            self.base.properties_updated = false;
        }
        layer_group
            .mutable_uniform_buffers()
            .set(ID_LINE_EVALUATED_PROPS_UBO, self.evaluated_props_uniform_buffer.clone());

        let mut i: u32 = 0;
        let count = layer_group.drawable_count();
        let mut drawable_ubo_vector = vec![LineDrawableUnionUBO::default(); count];
        let mut projection_ubo_vector = vec![ProjectionUBO::default(); count];
        let mut tile_props_ubo_vector = vec![LineTilePropsUnionUBO::default(); count];

        let key = self.base.key;
        layer_group.visit_drawables(|drawable| {
            const NEAR_CLIPPED: bool = false;
            const IN_VIEWPORT_PIXEL_UNITS: bool = false; // from RenderTile::translatedMatrix
            let shader = drawable.shader().cloned();
            let Some(tile_id) = drawable.tile_id else {
                return;
            };
            if shader.is_none() || !drawable.tweaked_by(key) {
                return;
            }

            let tile_id = tile_id.to_unwrapped();

            let Some(binders_slot) = drawable.binders.clone() else {
                debug_assert!(false);
                return;
            };
            let Some((bucket, layer_id)) = slot_bucket::<LineBucket>(&binders_slot) else {
                debug_assert!(false);
                return;
            };
            let Some(binders_lock) = bucket.paint_property_binders.get(layer_id) else {
                debug_assert!(false);
                return;
            };
            let binders_guard = binders_lock.read();
            let binders = &*binders_guard;
            let tile = drawable.render_tile.as_ref().map(DrawableRenderTile::tile);
            debug_assert!(tile.is_some());

            let pattern_pos_a = tile.and_then(|t| t.get_pattern(line_pattern_value.from.id()));
            let pattern_pos_b = tile.and_then(|t| t.get_pattern(line_pattern_value.to.id()));

            let translation = &evaluated.line_translate;
            let anchor = &evaluated.line_translate_anchor;
            let projection = get_projection_data(
                &tile_id,
                parameters,
                *translation.uniform(),
                *anchor.uniform(),
                NEAR_CLIPPED,
                IN_VIEWPORT_PIXEL_UNITS,
                drawable,
                false,
            );
            let matrix = projection.fallback_matrix;
            projection_ubo_vector[i as usize] = to_projection_ubo(&projection);

            match LineType::from_type(drawable.type_) {
                Some(LineType::Simple) => {
                    drawable_ubo_vector[i as usize].set(&LineDrawableUBO {
                        matrix: cast_matrix(&matrix),
                        ratio: pixel_scale / tile_id.pixels_to_tile_units(1.0, zoom),

                        color_t: binders.line_color.interpolation_factor(zoom),
                        blur_t: binders.line_blur.interpolation_factor(zoom),
                        opacity_t: binders.line_opacity.interpolation_factor(zoom),
                        gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                        offset_t: binders.line_offset.interpolation_factor(zoom),
                        width_t: binders.line_width.interpolation_factor(zoom),
                        pad1: 0.0,
                    });
                }

                Some(LineType::Gradient) => {
                    drawable_ubo_vector[i as usize].set(&LineGradientDrawableUBO {
                        matrix: cast_matrix(&matrix),
                        ratio: pixel_scale / tile_id.pixels_to_tile_units(1.0, zoom),

                        blur_t: binders.line_blur.interpolation_factor(zoom),
                        opacity_t: binders.line_opacity.interpolation_factor(zoom),
                        gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                        offset_t: binders.line_offset.interpolation_factor(zoom),
                        width_t: binders.line_width.interpolation_factor(zoom),
                        pad1: 0.0,
                        pad2: 0.0,
                    });
                }

                Some(LineType::Pattern) => {
                    let mut texture_size = Size::new(0, 0);
                    if let Some(texture) = drawable.textures[ID_LINE_IMAGE_TEXTURE].as_ref() {
                        texture_size = texture.borrow().size();
                    }
                    drawable_ubo_vector[i as usize].set(&LinePatternDrawableUBO {
                        matrix: cast_matrix(&matrix),
                        ratio: pixel_scale / tile_id.pixels_to_tile_units(1.0, zoom),

                        blur_t: binders.line_blur.interpolation_factor(zoom),
                        opacity_t: binders.line_opacity.interpolation_factor(zoom),
                        gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                        offset_t: binders.line_offset.interpolation_factor(zoom),
                        width_t: binders.line_width.interpolation_factor(zoom),
                        pattern_from_t: binders.line_pattern.interpolation_factor(zoom).0,
                        pattern_to_t: binders.line_pattern.interpolation_factor(zoom).1,
                        floorwidth_t: binders.line_floor_width.interpolation_factor(zoom),
                        pad1: 0.0,
                        pad2: 0.0,
                        pad3: 0.0,
                    });

                    tile_props_ubo_vector[i as usize].set(&LinePatternTilePropsUBO {
                        pattern_from: pattern_pos_a.as_ref().map_or([0.0; 4], |p| cast_tlbr(p.tlbr())),
                        pattern_to: pattern_pos_b.as_ref().map_or([0.0; 4], |p| cast_tlbr(p.tlbr())),
                        scale: [
                            parameters.pixel_ratio,
                            1.0 / tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom)),
                            crossfade.from_scale,
                            crossfade.to_scale,
                        ],
                        texsize: [texture_size.width as f32, texture_size.height as f32],
                        fade: crossfade.t,
                        pad1: 0.0,
                    });
                }

                Some(line_type @ (LineType::Sdf | LineType::GradientSdf)) => {
                    // The gradient variant keeps its ramp in the image slot,
                    // so the dash atlas takes the second.
                    let dash_texture_id =
                        if line_type == LineType::GradientSdf { ID_LINE_DASH_TEXTURE } else { ID_LINE_IMAGE_TEXTURE };
                    let tile_ratio = |scale: f32| 1.0 / tile_id.pixels_to_tile_units(scale, f32::from(int_zoom));
                    let sdf = if let Some(atlas) = binders.line_dasharray.atlas() {
                        // Per-feature patterns don't fade from the zoom
                        // before: both samples are of this zoom's pattern,
                        // at the cross-fade's two scales.
                        let atlas_size = atlas.image().size;
                        let min_scale = crossfade.from_scale.min(crossfade.to_scale);
                        Some(SdfPatterns {
                            patternscale_a: [tile_ratio(crossfade.from_scale), 0.0],
                            patternscale_b: [tile_ratio(crossfade.to_scale), 0.0],
                            tex_y_a: 0.0,
                            tex_y_b: 0.0,
                            atlas_height: atlas_size.height as f32,
                            sdfgamma: atlas_size.width as f32 / (min_scale * 256.0 * parameters.pixel_ratio) / 2.0,
                        })
                    } else if let Some(crate::drawable::DrawableData::Line(data)) = drawable.data {
                        let mut line_atlas = parameters.line_atlas.borrow_mut();
                        let dash_pattern_texture = line_atlas.get_dash_pattern_texture(
                            evaluated.line_dasharray.faded().from,
                            evaluated.line_dasharray.faded().to,
                            data.line_pattern_cap,
                        );

                        if drawable.textures[dash_texture_id].as_ref().is_none() {
                            let texture = dash_pattern_texture.get_texture();
                            drawable.enabled = texture.is_some();
                            if let Some(texture) = texture {
                                drawable.set_texture(Some(texture), dash_texture_id);
                            }
                        }

                        let pos_a = dash_pattern_texture.get_from();
                        let pos_b = dash_pattern_texture.get_to();
                        let width_a = pos_a.width * crossfade.from_scale;
                        let width_b = pos_b.width * crossfade.to_scale;
                        // std::min(widthA, widthB)
                        let min_width = if width_b < width_a { width_b } else { width_a };
                        Some(SdfPatterns {
                            patternscale_a: [tile_ratio(width_a), -pos_a.height / 2.0],
                            patternscale_b: [tile_ratio(width_b), -pos_b.height / 2.0],
                            tex_y_a: pos_a.y,
                            tex_y_b: pos_b.y,
                            atlas_height: 0.0,
                            sdfgamma: dash_pattern_texture.get_size().width as f32
                                / (min_width * 256.0 * parameters.pixel_ratio)
                                / 2.0,
                        })
                    } else {
                        None
                    };

                    if let Some(sdf) = sdf {
                        drawable_ubo_vector[i as usize].set(&LineSDFDrawableUBO {
                            matrix: cast_matrix(&matrix),
                            patternscale_a: sdf.patternscale_a,
                            patternscale_b: sdf.patternscale_b,
                            tex_y_a: sdf.tex_y_a,
                            tex_y_b: sdf.tex_y_b,
                            ratio: pixel_scale / tile_id.pixels_to_tile_units(1.0, zoom),

                            color_t: binders.line_color.interpolation_factor(zoom),
                            blur_t: binders.line_blur.interpolation_factor(zoom),
                            opacity_t: binders.line_opacity.interpolation_factor(zoom),
                            gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                            offset_t: binders.line_offset.interpolation_factor(zoom),
                            width_t: binders.line_width.interpolation_factor(zoom),
                            floorwidth_t: binders.line_floor_width.interpolation_factor(zoom),
                            atlas_height: sdf.atlas_height,
                            pad2: 0.0,
                        });

                        tile_props_ubo_vector[i as usize].set(&LineSDFTilePropsUBO {
                            sdfgamma: sdf.sdfgamma,
                            mix: crossfade.t,
                            pad1: 0.0,
                            pad2: 0.0,
                        });
                    }
                }

                None => {
                    Log::error(Event::General, format!("LineLayerTweaker: unknown line type: {}", drawable.type_));
                }
            }

            drawable.ubo_index = i;
            i += 1;
        });

        let bytes: &[u8] = bytemuck::cast_slice(&drawable_ubo_vector);
        context.write_storage_buffer(&mut self.drawable_uniform_buffer, bytes);

        let bytes: &[u8] = bytemuck::cast_slice(&tile_props_ubo_vector);
        context.write_storage_buffer(&mut self.tile_props_uniform_buffer, bytes);

        let layer_uniforms = layer_group.mutable_uniform_buffers();
        layer_uniforms.set(ID_LINE_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        layer_uniforms.set(ID_LINE_TILE_PROPS_UBO, self.tile_props_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_uniforms, &projection_ubo_vector, context);
    }
}
