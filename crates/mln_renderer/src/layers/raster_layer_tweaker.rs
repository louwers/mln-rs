// Ports src/mln/renderer/layers/raster_layer_tweaker.hpp and
// src/mln/renderer/layers/raster_layer_tweaker.cpp (MLN_UBO_CONSOLIDATION
// branches: the port's backend is WebGPU).

use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shaders::raster_layer_ubo::{RasterDrawableUBO, RasterEvaluatedPropsUBO};
use mln_gfx::shaders::shader_defines::{ID_RASTER_DRAWABLE_UBO, ID_RASTER_EVALUATED_PROPS_UBO};
use mln_gfx::{LayerGroupBase, UniformBufferPtr};
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::raster_layer_properties::*;
use mln_style_types::TranslateAnchorType;
use mln_util::immutable::Immutable;
use mln_util::logging::{Event, Log};
use mln_util::mat4::Mat4;

use super::{cast_matrix, check_tweak_drawable_ptr};
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_tile_matrix, multiply_with_projection_matrix};
use crate::paint_parameters::PaintParameters;

/// `gfx::ImageDrawableData`: the matrix of an image-source drawable
/// (include/mln/gfx/image_drawable_data.hpp).
pub struct ImageDrawableData {
    pub matrix: Mat4,
}

pub struct RasterLayerTweaker {
    base: LayerTweakerBase,
    evaluated_props_uniform_buffer: Option<UniformBufferPtr>,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
}

impl RasterLayerTweaker {
    pub fn new(id: String, properties: Immutable<dyn LayerProperties>) -> Self {
        RasterLayerTweaker {
            base: LayerTweakerBase::new(id, properties),
            evaluated_props_uniform_buffer: None,
            drawable_uniform_buffer: None,
        }
    }
}

fn spin_weights(spin: f32) -> [f32; 4] {
    let spin = mln_math::deg2radf(spin);
    let s = spin.sin();
    let c = spin.cos();
    let sqrt3 = 3.0f32.sqrt(); // std::numbers::sqrt3_v<float>
    [(2.0 * c + 1.0) / 3.0, (-sqrt3 * s - c + 1.0) / 3.0, (sqrt3 * s - c + 1.0) / 3.0, 0.0]
}

fn saturation_factor(saturation: f32) -> f32 {
    if saturation > 0.0 { 1.0 - 1.0 / (1.001 - saturation) } else { -saturation }
}

fn contrast_factor(contrast: f32) -> f32 {
    if contrast > 0.0 { 1.0 / (1.0 - contrast) } else { 1.0 + contrast }
}

impl LayerTweaker for RasterLayerTweaker {
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
        let evaluated = get_evaluated::<RasterLayerProperties>(&properties);

        if self.evaluated_props_uniform_buffer.is_none() || self.base.properties_updated {
            let props_ubo = RasterEvaluatedPropsUBO {
                spin_weights: spin_weights(*evaluated.get::<RasterHueRotate>()),
                tl_parent: [0.0, 0.0],
                scale_parent: 1.0,
                buffer_scale: 1.0,
                fade_t: 1.0,
                opacity: *evaluated.get::<RasterOpacity>(),
                brightness_low: *evaluated.get::<RasterBrightnessMin>(),
                brightness_high: *evaluated.get::<RasterBrightnessMax>(),
                saturation_factor: saturation_factor(*evaluated.get::<RasterSaturation>()),
                contrast_factor: contrast_factor(*evaluated.get::<RasterContrast>()),
                pad1: 0.0,
                pad2: 0.0,
            };
            context.emplace_or_update_uniform_buffer_pod(&mut self.evaluated_props_uniform_buffer, &props_ubo);
            self.base.properties_updated = false;
        }
        layer_group
            .mutable_uniform_buffers()
            .set(ID_RASTER_EVALUATED_PROPS_UBO, self.evaluated_props_uniform_buffer.clone());

        let mut i: u32 = 0;
        let mut drawable_ubo_vector = vec![RasterDrawableUBO::default(); layer_group.get_drawable_count()];

        let self_ptr: *const Self = self;
        visit_layer_group_drawables(layer_group, |drawable| {
            if !check_tweak_drawable_ptr(drawable, self_ptr) {
                return;
            }

            let matrix: Mat4 = match drawable.get_tile_id() {
                None => {
                    // this is an image drawable
                    let Some(image_data) = drawable.get_data().and_then(|d| d.downcast_ref::<ImageDrawableData>())
                    else {
                        Log::error(
                            Event::General,
                            "Invalid raster layer drawable: neither tile id nor image data is set.",
                        );
                        return;
                    };
                    let mut matrix = image_data.matrix;
                    multiply_with_projection_matrix(
                        /*in-out*/ &mut matrix,
                        parameters.transform_params,
                        /*near_clipped*/ false,
                        /*aligned*/ true,
                    );
                    matrix
                }
                Some(tile_id) => {
                    // this is a tile drawable
                    let tile_id = tile_id.to_unwrapped();
                    get_tile_matrix(
                        &tile_id,
                        parameters,
                        &[0.0, 0.0],
                        TranslateAnchorType::Viewport,
                        false,
                        false,
                        drawable,
                        !parameters.state.is_changing(),
                    )
                }
            };

            drawable_ubo_vector[i as usize] = RasterDrawableUBO { matrix: cast_matrix(&matrix) };
            drawable.set_ubo_index(i);
            i += 1;
        });

        let bytes: &[u8] = bytemuck::cast_slice(&drawable_ubo_vector);
        match &self.drawable_uniform_buffer {
            Some(buffer) if buffer.get_size() >= bytes.len() => buffer.update(bytes),
            _ => {
                self.drawable_uniform_buffer = Some(context.create_uniform_buffer(bytes, false, true));
            }
        }

        layer_group.mutable_uniform_buffers().set(ID_RASTER_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
    }
}
