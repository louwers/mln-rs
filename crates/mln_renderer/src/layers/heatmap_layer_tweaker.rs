use std::sync::Arc;

use mln_gfx::shaders::heatmap_layer_ubo::{HeatmapDrawableUBO, HeatmapEvaluatedPropsUBO};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{ID_HEATMAP_DRAWABLE_UBO, ID_HEATMAP_EVALUATED_PROPS_UBO};
use mln_style::layers::{Heatmap, HeatmapPaintProperties};
use mln_style_types::TranslateAnchorType;

use super::{LayerTiles, cast_matrix};
use crate::gfx::uniform_buffer::UniformBufferPtr;
use crate::layer_group::LayerGroup;
use crate::layer_tweaker::{
    LayerTweaker, LayerTweakerBase, get_projection_data, globe_extrude_scale, to_projection_ubo,
};
use crate::layer_type::{ErasedEvaluatedLayer, evaluated};
use crate::paint_parameters::PaintParameters;

pub(crate) struct HeatmapLayerTweaker {
    base: LayerTweakerBase,
    evaluated_props_uniform_buffer: Option<UniformBufferPtr>,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
}

impl HeatmapLayerTweaker {
    pub(crate) fn new(id: String, properties: Arc<dyn ErasedEvaluatedLayer>) -> Self {
        HeatmapLayerTweaker {
            base: LayerTweakerBase::new(id, Some(properties)),
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

    fn execute(&mut self, layer_group: &mut LayerGroup, tiles: &LayerTiles<'_>, parameters: &PaintParameters<'_>) {
        if layer_group.empty() {
            return;
        }

        let context = parameters.context;
        let zoom = parameters.state.get_zoom() as f32;
        let Some(properties) = self.base.evaluated_properties.clone() else {
            return;
        };
        let evaluated = &evaluated::<Heatmap>(&properties).paint;

        if self.evaluated_props_uniform_buffer.is_none() || self.base.properties_updated {
            let evaluated_props_ubo = HeatmapEvaluatedPropsUBO {
                weight: evaluated.heatmap_weight.constant_or(&HeatmapPaintProperties::Defaults::heatmap_weight()),
                radius: evaluated.heatmap_radius.constant_or(&HeatmapPaintProperties::Defaults::heatmap_radius()),
                intensity: *evaluated.heatmap_intensity.uniform(),
                padding: 0.0,
            };
            context.write_uniform_buffer_pod(&mut self.evaluated_props_uniform_buffer, &evaluated_props_ubo);
            self.base.properties_updated = false;
        }
        layer_group
            .mutable_uniform_buffers()
            .set(ID_HEATMAP_EVALUATED_PROPS_UBO, self.evaluated_props_uniform_buffer.clone());

        let mut i: u32 = 0;
        let mut drawable_ubo_vector = vec![HeatmapDrawableUBO::default(); layer_group.drawable_count()];
        let mut projection_ubo_vector = vec![ProjectionUBO::default(); layer_group.drawable_count()];

        let latitude_scale = parameters.state.get_projection().circle_radius_correction(parameters.state);
        let key = self.base.key;
        let layer_id = self.base.id.clone();
        layer_group.visit_drawables(|drawable| {
            const NEAR_CLIPPED: bool = false;
            const IN_VIEWPORT_PIXEL_UNITS: bool = false;
            let Some(tile_id) = drawable.tile_id else {
                return;
            };
            if !drawable.tweaked_by(key) {
                return;
            }
            let Some((_, bucket)) = tiles.bucket::<Heatmap>(&tile_id) else {
                debug_assert!(false);
                return;
            };
            let tile_id = tile_id.to_unwrapped();
            let Some(binders_lock) = bucket.paint_property_binders.get(&layer_id) else {
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
                drawable.origin,
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
            drawable.ubo_index = i;
            i += 1;
        });

        let bytes: &[u8] = bytemuck::cast_slice(&drawable_ubo_vector);
        context.write_storage_buffer(&mut self.drawable_uniform_buffer, bytes);

        layer_group.mutable_uniform_buffers().set(ID_HEATMAP_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_group.mutable_uniform_buffers(), &projection_ubo_vector, context);
    }
}
