// Ports src/mln/renderer/layers/circle_layer_tweaker.hpp and
// src/mln/renderer/layers/circle_layer_tweaker.cpp (MLN_UBO_CONSOLIDATION
// branches: the port's backend is WebGPU).

use std::sync::Arc;

use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shaders::circle_layer_ubo::{CircleDrawableUBO, CircleEvaluatedPropsUBO};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{ID_CIRCLE_DRAWABLE_UBO, ID_CIRCLE_EVALUATED_PROPS_UBO};
use mln_gfx::{LayerGroupBase, UniformBufferPtr};
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::circle_layer_properties::{
    CircleBlur, CircleColor, CircleLayerProperties, CircleOpacity, CirclePitchAlignment, CirclePitchScale,
    CircleRadius, CircleStrokeColor, CircleStrokeOpacity, CircleStrokeWidth, CircleTranslate, CircleTranslateAnchor,
};
use mln_style::properties::Property;
use mln_style_types::{AlignmentType, CirclePitchScaleType};
use mln_util::sync::RwLockExt;

use super::{cast_matrix, check_tweak_drawable_ptr, get_drawable_binders, slot_bucket};
use crate::buckets::circle_bucket::CircleBucket;
use crate::layer_tweaker::{
    LayerTweaker, LayerTweakerBase, get_projection_data, globe_extrude_scale, to_projection_ubo,
};
use crate::paint_parameters::PaintParameters;

pub struct CircleLayerTweaker {
    base: LayerTweakerBase,
    evaluated_props_uniform_buffer: Option<UniformBufferPtr>,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
}

impl CircleLayerTweaker {
    pub fn new(id: String, properties: Arc<dyn LayerProperties>) -> Self {
        CircleLayerTweaker {
            base: LayerTweakerBase::new(id, properties),
            evaluated_props_uniform_buffer: None,
            drawable_uniform_buffer: None,
        }
    }
}

/// `constOrDefault<P>(evaluated)`: `evaluated.get<P>().constantOr(P::defaultValue())`.
macro_rules! const_or_default {
    ($evaluated:expr, $p:ty) => {
        $evaluated.get::<$p>().constant_or(&<$p>::default_value())
    };
}

impl LayerTweaker for CircleLayerTweaker {
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
        let evaluated = get_evaluated::<CircleLayerProperties>(&properties);

        let zoom = parameters.state.get_zoom() as f32;
        let pitch_with_map = *evaluated.get::<CirclePitchAlignment>() == AlignmentType::Map;
        let scale_with_map = *evaluated.get::<CirclePitchScale>() == CirclePitchScaleType::Map;

        // Updated only with evaluated properties
        if self.evaluated_props_uniform_buffer.is_none() || self.base.properties_updated {
            let evaluated_props_ubo = CircleEvaluatedPropsUBO {
                color: const_or_default!(evaluated, CircleColor).to_float_array(),
                stroke_color: const_or_default!(evaluated, CircleStrokeColor).to_float_array(),
                radius: const_or_default!(evaluated, CircleRadius),
                blur: const_or_default!(evaluated, CircleBlur),
                opacity: const_or_default!(evaluated, CircleOpacity),
                stroke_width: const_or_default!(evaluated, CircleStrokeWidth),
                stroke_opacity: const_or_default!(evaluated, CircleStrokeOpacity),
                scale_with_map: i32::from(scale_with_map),
                pitch_with_map: i32::from(pitch_with_map),
                pad1: 0.0,
            };
            context
                .emplace_or_update_uniform_buffer_pod(&mut self.evaluated_props_uniform_buffer, &evaluated_props_ubo);
            self.base.properties_updated = false;
        }
        layer_group
            .mutable_uniform_buffers()
            .set(ID_CIRCLE_EVALUATED_PROPS_UBO, self.evaluated_props_uniform_buffer.clone());

        let mut i: u32 = 0;
        let mut drawable_ubo_vector = vec![CircleDrawableUBO::default(); layer_group.get_drawable_count()];
        let mut projection_ubo_vector = vec![ProjectionUBO::default(); layer_group.get_drawable_count()];

        let latitude_scale = parameters.state.get_projection().circle_radius_correction(parameters.state);
        let self_ptr: *const Self = self;
        visit_layer_group_drawables(layer_group, |drawable| {
            const IN_VIEWPORT_PIXEL_UNITS: bool = false; // from RenderTile::translatedMatrix
            const NEAR_CLIPPED: bool = false;
            debug_assert!(drawable.get_tile_id().is_some(), "Circles only render with tiles");
            let Some(tile_id) = drawable.get_tile_id() else {
                return;
            };
            if !check_tweak_drawable_ptr(drawable, self_ptr) {
                return;
            }
            let tile_id = tile_id.to_unwrapped();

            let Some(binders_slot) = get_drawable_binders(drawable) else {
                debug_assert!(false);
                return;
            };
            let Some((bucket, layer_id)) = slot_bucket::<CircleBucket>(&binders_slot) else {
                debug_assert!(false);
                return;
            };
            let Some(binders_lock) = bucket.paint_property_binders.get(layer_id) else {
                debug_assert!(false);
                return;
            };
            let binders_guard = binders_lock.read_unpoisoned();
            let binders = &*binders_guard;

            let translation = evaluated.get::<CircleTranslate>();
            let anchor = *evaluated.get::<CircleTranslateAnchor>();
            let projection = get_projection_data(
                &tile_id,
                parameters,
                translation,
                anchor,
                NEAR_CLIPPED,
                IN_VIEWPORT_PIXEL_UNITS,
                drawable,
                false,
            );
            let matrix = projection.fallback_matrix;
            projection_ubo_vector[i as usize] = to_projection_ubo(&projection);

            let pixels_to_tile_units = tile_id.pixels_to_tile_units(1.0, zoom);
            let globe_extrude_scale = globe_extrude_scale(&tile_id, zoom, latitude_scale);
            let extrude_scale = if pitch_with_map {
                [pixels_to_tile_units, pixels_to_tile_units]
            } else {
                parameters.pixels_to_gl_units
            };

            drawable_ubo_vector[i as usize] = CircleDrawableUBO {
                matrix: cast_matrix(&matrix),
                extrude_scale,

                color_t: binders.circle_color.interpolation_factor(zoom),
                radius_t: binders.circle_radius.interpolation_factor(zoom),
                blur_t: binders.circle_blur.interpolation_factor(zoom),
                opacity_t: binders.circle_opacity.interpolation_factor(zoom),
                stroke_color_t: binders.circle_stroke_color.interpolation_factor(zoom),
                stroke_width_t: binders.circle_stroke_width.interpolation_factor(zoom),
                stroke_opacity_t: binders.circle_stroke_opacity.interpolation_factor(zoom),
                globe_extrude_scale,
                pad2: 0.0,
                pad3: 0.0,
            };
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

        layer_group.mutable_uniform_buffers().set(ID_CIRCLE_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_group.mutable_uniform_buffers(), &projection_ubo_vector, context);
    }
}
