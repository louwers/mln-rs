use std::sync::Arc;

use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shaders::heatmap_layer_ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{ID_HEATMAP_DRAWABLE_UBO, ID_HEATMAP_EVALUATED_PROPS_UBO};
use mln_gfx::{LayerGroupBase, UniformBufferPtr};
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::heatmap_layer_properties::{
    HeatmapIntensity, HeatmapLayerProperties, HeatmapRadius, HeatmapWeight,
};
use mln_style::properties::Property;
use mln_style_types::TranslateAnchorType;

use super::{cast_matrix, check_tweak_drawable_ptr, get_drawable_binders, slot_bucket};
use crate::buckets::heatmap_bucket::HeatmapBucket;
use crate::layer_tweaker::{
    LayerTweaker, LayerTweakerBase, get_projection_data, globe_extrude_scale, to_projection_ubo,
};
use crate::paint_parameters::PaintParameters;

pub(crate) struct HeatmapLayerTweaker {
    base: LayerTweakerBase,
    evaluated_props_uniform_buffer: Option<UniformBufferPtr>,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
}

impl HeatmapLayerTweaker {
    pub(crate) fn new(id: String, properties: Arc<dyn LayerProperties>) -> Self {
        HeatmapLayerTweaker {
            base: LayerTweakerBase::new(id, properties),
            evaluated_props_uniform_buffer: None,
            drawable_uniform_buffer: None,
        }
    }
}

impl LayerTweaker for HeatmapLayerTweaker {
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
        let zoom = parameters.state.get_zoom() as f32;
        let properties = self.base.evaluated_properties.clone();
        let evaluated = get_evaluated::<HeatmapLayerProperties>(&properties);

        if self.evaluated_props_uniform_buffer.is_none() || self.base.properties_updated {
            let evaluated_props_ubo = HeatmapEvaluatedPropsUBO {
                weight: evaluated.get::<HeatmapWeight>().constant_or(&HeatmapWeight::default_value()),
                radius: evaluated.get::<HeatmapRadius>().constant_or(&HeatmapRadius::default_value()),
                intensity: *evaluated.get::<HeatmapIntensity>(),
                padding: 0.0,
            };
            context
                .emplace_or_update_uniform_buffer_pod(&mut self.evaluated_props_uniform_buffer, &evaluated_props_ubo);
            self.base.properties_updated = false;
        }
        layer_group
            .mutable_uniform_buffers()
            .set(ID_HEATMAP_EVALUATED_PROPS_UBO, self.evaluated_props_uniform_buffer.clone());

        let mut i: u32 = 0;
        let mut drawable_ubo_vector = vec![HeatmapDrawableUBO::default(); layer_group.get_drawable_count()];
        let mut projection_ubo_vector = vec![ProjectionUBO::default(); layer_group.get_drawable_count()];

        let latitude_scale = parameters.state.get_projection().circle_radius_correction(parameters.state);
        let self_ptr: *const Self = self;
        visit_layer_group_drawables(layer_group, |drawable| {
            const NEAR_CLIPPED: bool = false;
            const IN_VIEWPORT_PIXEL_UNITS: bool = false;
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
            if drawable.get_render_tile().is_none() {
                debug_assert!(false);
                return;
            }
            let Some((bucket, layer_id)) = slot_bucket::<HeatmapBucket>(&binders_slot) else {
                debug_assert!(false);
                return;
            };
            let Some(binders_lock) = bucket.paint_property_binders.get(layer_id) else {
                debug_assert!(false);
                return;
            };
            let binders = binders_lock.read();

            let projection = get_projection_data(
                &tile_id,
                parameters,
                [0.0, 0.0],
                TranslateAnchorType::Viewport,
                NEAR_CLIPPED,
                IN_VIEWPORT_PIXEL_UNITS,
                drawable,
                false,
            );
            let matrix = projection.fallback_matrix;
            projection_ubo_vector[i as usize] = to_projection_ubo(&projection);

            drawable_ubo_vector[i as usize] = HeatmapDrawableUBO {
                matrix: cast_matrix(&matrix),
                extrude_scale: tile_id.pixels_to_tile_units(1.0, zoom),

                weight_t: binders.heatmap_weight.interpolation_factor(zoom),
                radius_t: binders.heatmap_radius.interpolation_factor(zoom),
                globe_extrude_scale: globe_extrude_scale(&tile_id, zoom, latitude_scale),
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

        layer_group.mutable_uniform_buffers().set(ID_HEATMAP_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_group.mutable_uniform_buffers(), &projection_ubo_vector, context);
    }
}
