use std::sync::Arc;

use mln_gfx::shaders::color_relief_layer_ubo::{
    ColorReliefDrawableUBO, ColorReliefEvaluatedPropsUBO, ColorReliefTilePropsUBO,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{
    ID_COLOR_RELIEF_DRAWABLE_UBO, ID_COLOR_RELIEF_EVALUATED_PROPS_UBO, ID_COLOR_RELIEF_TILE_PROPS_UBO,
};
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::color_relief_layer_properties::{ColorReliefLayerProperties, ColorReliefOpacity};

use super::cast_matrix;
use crate::gfx::uniform_buffer::UniformBufferPtr;
use crate::layer_group::LayerGroup;
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, to_projection_ubo};
use crate::paint_parameters::PaintParameters;

/// Per-tile DEM properties that are set during drawable creation and read by
/// the tweaker during rendering.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ColorReliefDrawableData {
    pub(crate) tile_props: ColorReliefTilePropsUBO,
}

pub(crate) struct ColorReliefLayerTweaker {
    base: LayerTweakerBase,
    evaluated_props_uniform_buffer: Option<UniformBufferPtr>,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
    tile_props_uniform_buffer: Option<UniformBufferPtr>,
}

impl ColorReliefLayerTweaker {
    pub(crate) fn new(id: String, properties: Arc<dyn LayerProperties>) -> Self {
        ColorReliefLayerTweaker {
            base: LayerTweakerBase::new(id, properties),
            evaluated_props_uniform_buffer: None,
            drawable_uniform_buffer: None,
            tile_props_uniform_buffer: None,
        }
    }
}

impl LayerTweaker for ColorReliefLayerTweaker {
    fn base(&self) -> &LayerTweakerBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut LayerTweakerBase {
        &mut self.base
    }

    fn execute(&mut self, layer_group: &mut LayerGroup, parameters: &PaintParameters<'_>) {
        let context = parameters.context;
        let properties = Arc::clone(&self.base.evaluated_properties);
        let evaluated = get_evaluated::<ColorReliefLayerProperties>(&properties);

        if layer_group.empty() {
            return;
        }

        // Update evaluated properties UBO
        let evaluated_props_ubo = ColorReliefEvaluatedPropsUBO {
            opacity: *evaluated.get::<ColorReliefOpacity>(),
            pad_eval0: 0.0,
            pad_eval1: 0.0,
            pad_eval2: 0.0,
        };
        context.write_uniform_buffer_pod(&mut self.evaluated_props_uniform_buffer, &evaluated_props_ubo);

        layer_group
            .mutable_uniform_buffers()
            .set(ID_COLOR_RELIEF_EVALUATED_PROPS_UBO, self.evaluated_props_uniform_buffer.clone());

        let mut i: u32 = 0;
        let count = layer_group.drawable_count();
        let mut drawable_ubo_vector = vec![ColorReliefDrawableUBO::default(); count];
        let mut tile_props_ubo_vector = vec![ColorReliefTilePropsUBO::default(); count];
        let mut projection_ubo_vector = vec![ProjectionUBO::default(); count];

        layer_group.visit_drawables(|drawable| {
            let Some(tile_id) = drawable.tile_id else {
                return;
            };

            let tile_id = tile_id.to_unwrapped();

            let projection = parameters.projection_data_for_tile(&tile_id, false);

            drawable_ubo_vector[i as usize].matrix = cast_matrix(&projection.fallback_matrix);
            projection_ubo_vector[i as usize] = to_projection_ubo(&projection);

            // Get tile props from drawable data (set during creation)
            if let Some(crate::drawable::DrawableData::ColorRelief(data)) = &drawable.data {
                tile_props_ubo_vector[i as usize] = data.tile_props;
            }

            drawable.ubo_index = i;
            i += 1;
        });

        let bytes: &[u8] = bytemuck::cast_slice(&drawable_ubo_vector);
        context.write_storage_buffer(&mut self.drawable_uniform_buffer, bytes);

        let bytes: &[u8] = bytemuck::cast_slice(&tile_props_ubo_vector);
        context.write_storage_buffer(&mut self.tile_props_uniform_buffer, bytes);

        let layer_uniforms = layer_group.mutable_uniform_buffers();
        layer_uniforms.set(ID_COLOR_RELIEF_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        layer_uniforms.set(ID_COLOR_RELIEF_TILE_PROPS_UBO, self.tile_props_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_uniforms, &projection_ubo_vector, context);
    }
}
