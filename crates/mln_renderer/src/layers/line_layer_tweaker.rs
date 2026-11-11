// Ports src/mln/renderer/layers/line_layer_tweaker.hpp and
// src/mln/renderer/layers/line_layer_tweaker.cpp (MLN_RENDER_BACKEND_METAL,
// MLN_RENDER_BACKEND_WEBGPU and MLN_UBO_CONSOLIDATION branches, the backend
// picked by the context).
//
// TODO(port): the GPU expressions of the Metal branch
// (`updateGPUExpressions`, `gfx::Backend::getEnableGPUExpressionEval`), off
// by default in C++: on Metal the expression UBO is all zeros and no
// property is marked as evaluated on the GPU.

use mln_gfx::backend::Type as BackendType;
use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::line_layer_ubo::*;
use mln_gfx::shaders::shader_defines::{
    ID_LINE_DASH_TEXTURE, ID_LINE_DRAWABLE_UBO, ID_LINE_EVALUATED_PROPS_UBO, ID_LINE_EXPRESSION_UBO,
    ID_LINE_IMAGE_TEXTURE, ID_LINE_TILE_PROPS_UBO,
};
use mln_gfx::{LayerGroupBase, UniformBufferPtr};
use mln_style::expression::Image;
use mln_style::layer_properties::{LayerProperties, get_crossfade, get_evaluated};
use mln_style::layers::line_layer_properties::*;
use mln_style::properties::Property;
use mln_style::renderer::cross_faded_property_evaluator::Faded;
use mln_util::immutable::Immutable;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;
use mln_util::sync::RwLockExt;

use super::{cast_matrix, check_tweak_drawable_ptr, get_drawable_binders, get_drawable_render_tile, slot_bucket};
use crate::buckets::line_bucket::LineBucket;
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_projection_data, to_projection_ubo};
use crate::line_atlas::LinePatternCap;
use crate::paint_parameters::PaintParameters;

/// `LineLayerTweaker::LineType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LineType {
    Simple,
    Gradient,
    Pattern,
    SDF,
    GradientSDF,
}

impl LineType {
    pub fn from_type(type_: usize) -> Option<LineType> {
        match type_ {
            0 => Some(LineType::Simple),
            1 => Some(LineType::Gradient),
            2 => Some(LineType::Pattern),
            3 => Some(LineType::SDF),
            4 => Some(LineType::GradientSDF),
            _ => None,
        }
    }
}

/// `gfx::LineDrawableData` (gfx/line_drawable_data.hpp): the line cap the
/// dash pattern was built for.
#[derive(Clone, Copy, Debug)]
pub struct LineDrawableData {
    pub line_pattern_cap: LinePatternCap,
}

pub struct LineLayerTweaker {
    base: LayerTweakerBase,
    evaluated_props_uniform_buffer: Option<UniformBufferPtr>,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
    tile_props_uniform_buffer: Option<UniformBufferPtr>,
    /// Metal: the `LineExpressionUBO`.
    expression_uniform_buffer: Option<UniformBufferPtr>,
    expression_mask: u32,
}

impl LineLayerTweaker {
    pub fn new(id: String, properties: Immutable<dyn LayerProperties>) -> Self {
        LineLayerTweaker {
            base: LayerTweakerBase::new(id, properties),
            evaluated_props_uniform_buffer: None,
            drawable_uniform_buffer: None,
            tile_props_uniform_buffer: None,
            expression_uniform_buffer: None,
            expression_mask: line_expression_mask::NONE,
        }
    }
}

fn cast_tlbr(a: [u16; 4]) -> [f32; 4] {
    [a[0] as f32, a[1] as f32, a[2] as f32, a[3] as f32]
}

impl LayerTweaker for LineLayerTweaker {
    fn base(&self) -> &LayerTweakerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut LayerTweakerBase {
        &mut self.base
    }

    fn execute(&mut self, layer_group: &mut dyn LayerGroupBase, parameters: &PaintParameters<'_>) {
        if layer_group.empty() {
            return;
        }

        let context = parameters.context;
        let properties = self.base.evaluated_properties.clone();
        let evaluated = get_evaluated::<LineLayerProperties>(&properties);
        let crossfade = *get_crossfade::<LineLayerProperties>(&properties);

        let line_pattern_value =
            evaluated.get::<LinePattern>().constant_or(&Faded { from: Image::new(""), to: Image::new("") });

        let zoom = parameters.state.get_zoom() as f32;
        let int_zoom = parameters.state.get_integer_zoom();
        let state = parameters.state;
        let pixel_scale = state.get_projection().pixel_scale(state) as f32;

        let metal = context.backend_type() == BackendType::Metal;
        if self.evaluated_props_uniform_buffer.is_none() || self.base.properties_updated {
            use line_expression_mask::*;
            self.expression_mask = NONE;
            // On Metal, the mask marks the properties evaluated on the GPU
            // (none, see the top of the file); on WebGPU, the constant ones.
            if !metal {
                if evaluated.get::<LineColor>().is_constant() {
                    self.expression_mask |= COLOR;
                }
                if evaluated.get::<LineBlur>().is_constant() {
                    self.expression_mask |= BLUR;
                }
                if evaluated.get::<LineOpacity>().is_constant() {
                    self.expression_mask |= OPACITY;
                }
                if evaluated.get::<LineGapWidth>().is_constant() {
                    self.expression_mask |= GAP_WIDTH;
                }
                if evaluated.get::<LineOffset>().is_constant() {
                    self.expression_mask |= OFFSET;
                }
                if evaluated.get::<LineWidth>().is_constant() {
                    self.expression_mask |= WIDTH;
                }
                if evaluated.get::<LineFloorWidth>().is_constant() {
                    self.expression_mask |= FLOOR_WIDTH;
                }
            }

            let props_ubo = LineEvaluatedPropsUBO {
                color: evaluated.get::<LineColor>().constant_or(&LineColor::default_value()).to_float_array(),
                blur: evaluated.get::<LineBlur>().constant_or(&LineBlur::default_value()),
                opacity: evaluated.get::<LineOpacity>().constant_or(&LineOpacity::default_value()),
                gapwidth: evaluated.get::<LineGapWidth>().constant_or(&LineGapWidth::default_value()),
                offset: evaluated.get::<LineOffset>().constant_or(&LineOffset::default_value()),
                width: evaluated.get::<LineWidth>().constant_or(&LineWidth::default_value()),
                floorwidth: evaluated.get::<LineFloorWidth>().constant_or(&LineFloorWidth::default_value()),
                expression_mask: self.expression_mask,
                pad1: 0.0,
            };
            context.emplace_or_update_uniform_buffer_pod(&mut self.evaluated_props_uniform_buffer, &props_ubo);
            self.base.properties_updated = false;
        }
        layer_group
            .mutable_uniform_buffers()
            .set(ID_LINE_EVALUATED_PROPS_UBO, self.evaluated_props_uniform_buffer.clone());
        if metal {
            if self.expression_uniform_buffer.is_none() {
                self.expression_uniform_buffer =
                    Some(context.create_uniform_buffer(&[0; LINE_EXPRESSION_UBO_SIZE], false, false));
            }
            layer_group.mutable_uniform_buffers().set(ID_LINE_EXPRESSION_UBO, self.expression_uniform_buffer.clone());
        }

        let mut i: u32 = 0;
        let count = layer_group.get_drawable_count();
        let mut drawable_ubo_vector = vec![LineDrawableUnionUBO::default(); count];
        let mut projection_ubo_vector = vec![ProjectionUBO::default(); count];
        let mut tile_props_ubo_vector = vec![LineTilePropsUnionUBO::default(); count];

        let self_ptr: *const Self = self;
        visit_layer_group_drawables(layer_group, |drawable| {
            let shader = drawable.get_shader().clone();
            let Some(tile_id) = drawable.get_tile_id() else {
                return;
            };
            if shader.is_none() || !check_tweak_drawable_ptr(drawable, self_ptr) {
                return;
            }

            let tile_id = tile_id.to_unwrapped();

            let Some(binders_slot) = get_drawable_binders(drawable) else {
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
            let binders_guard = binders_lock.read_unpoisoned();
            let binders = &*binders_guard;
            let tile = get_drawable_render_tile(drawable);
            debug_assert!(tile.is_some());

            let pattern_pos_a = tile.and_then(|t| t.get_pattern(line_pattern_value.from.id()));
            let pattern_pos_b = tile.and_then(|t| t.get_pattern(line_pattern_value.to.id()));
            binders.set_pattern_parameters(pattern_pos_a.as_ref(), pattern_pos_b.as_ref(), &crossfade);

            let translation = *evaluated.get::<LineTranslate>();
            let anchor = *evaluated.get::<LineTranslateAnchor>();
            const NEAR_CLIPPED: bool = false;
            const IN_VIEWPORT_PIXEL_UNITS: bool = false; // from RenderTile::translatedMatrix
            let projection = get_projection_data(
                &tile_id,
                parameters,
                &translation,
                anchor,
                NEAR_CLIPPED,
                IN_VIEWPORT_PIXEL_UNITS,
                drawable,
                false,
            );
            let matrix = projection.fallback_matrix;
            projection_ubo_vector[i as usize] = to_projection_ubo(&projection);

            match LineType::from_type(drawable.get_type()) {
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
                    if let Some(texture) = drawable.get_texture(ID_LINE_IMAGE_TEXTURE) {
                        texture_size = texture.borrow().get_size();
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
                            1.0 / tile_id.pixels_to_tile_units(1.0, int_zoom as f32),
                            crossfade.from_scale,
                            crossfade.to_scale,
                        ],
                        texsize: [texture_size.width as f32, texture_size.height as f32],
                        fade: crossfade.t,
                        pad1: 0.0,
                    });
                }

                Some(line_type @ (LineType::SDF | LineType::GradientSDF)) => {
                    let line_pattern_cap = drawable
                        .get_data()
                        .and_then(|d| d.downcast_ref::<LineDrawableData>())
                        .map(|d| d.line_pattern_cap);
                    if let Some(cap) = line_pattern_cap {
                        let mut line_atlas = parameters.line_atlas.borrow_mut();
                        let dash_pattern_texture = line_atlas.get_dash_pattern_texture(
                            &evaluated.get::<LineDasharray>().from,
                            &evaluated.get::<LineDasharray>().to,
                            cap,
                        );

                        // The gradient variant keeps its ramp in the image
                        // slot, so the dash atlas takes the second.
                        let dash_texture_id = if line_type == LineType::GradientSDF {
                            ID_LINE_DASH_TEXTURE
                        } else {
                            ID_LINE_IMAGE_TEXTURE
                        };
                        if drawable.get_texture(dash_texture_id).is_none() {
                            let texture = dash_pattern_texture.get_texture();
                            drawable.set_enabled(texture.is_some());
                            if let Some(texture) = texture {
                                drawable.set_texture(Some(texture), dash_texture_id);
                            }
                        }

                        let pos_a = dash_pattern_texture.get_from();
                        let pos_b = dash_pattern_texture.get_to();
                        let width_a = pos_a.width * crossfade.from_scale;
                        let width_b = pos_b.width * crossfade.to_scale;

                        drawable_ubo_vector[i as usize].set(&LineSDFDrawableUBO {
                            matrix: cast_matrix(&matrix),
                            patternscale_a: [
                                1.0 / tile_id.pixels_to_tile_units(width_a, int_zoom as f32),
                                -pos_a.height / 2.0,
                            ],
                            patternscale_b: [
                                1.0 / tile_id.pixels_to_tile_units(width_b, int_zoom as f32),
                                -pos_b.height / 2.0,
                            ],
                            tex_y_a: pos_a.y,
                            tex_y_b: pos_b.y,
                            ratio: pixel_scale / tile_id.pixels_to_tile_units(1.0, zoom),

                            color_t: binders.line_color.interpolation_factor(zoom),
                            blur_t: binders.line_blur.interpolation_factor(zoom),
                            opacity_t: binders.line_opacity.interpolation_factor(zoom),
                            gapwidth_t: binders.line_gap_width.interpolation_factor(zoom),
                            offset_t: binders.line_offset.interpolation_factor(zoom),
                            width_t: binders.line_width.interpolation_factor(zoom),
                            floorwidth_t: binders.line_floor_width.interpolation_factor(zoom),
                            pad1: 0.0,
                            pad2: 0.0,
                        });

                        // std::min(widthA, widthB)
                        let min_width = if width_b < width_a { width_b } else { width_a };
                        tile_props_ubo_vector[i as usize].set(&LineSDFTilePropsUBO {
                            sdfgamma: dash_pattern_texture.get_size().width as f32
                                / (min_width * 256.0 * parameters.pixel_ratio)
                                / 2.0,
                            mix: crossfade.t,
                            pad1: 0.0,
                            pad2: 0.0,
                        });
                    }
                }

                None => {
                    Log::error(Event::General, format!("LineLayerTweaker: unknown line type: {}", drawable.get_type()));
                }
            }

            drawable.set_ubo_index(i);
            i += 1;
        });

        let bytes: &[u8] = bytemuck::cast_slice(&drawable_ubo_vector);
        match &self.drawable_uniform_buffer {
            Some(buffer) if buffer.get_size() >= bytes.len() => buffer.update(bytes),
            _ => self.drawable_uniform_buffer = Some(context.create_uniform_buffer(bytes, false, true)),
        }

        let bytes: &[u8] = bytemuck::cast_slice(&tile_props_ubo_vector);
        match &self.tile_props_uniform_buffer {
            Some(buffer) if buffer.get_size() >= bytes.len() => buffer.update(bytes),
            _ => self.tile_props_uniform_buffer = Some(context.create_uniform_buffer(bytes, false, true)),
        }

        let layer_uniforms = layer_group.mutable_uniform_buffers();
        layer_uniforms.set(ID_LINE_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        layer_uniforms.set(ID_LINE_TILE_PROPS_UBO, self.tile_props_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_uniforms, &projection_ubo_vector, context);
    }
}
