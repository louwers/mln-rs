use std::sync::Arc;

use mln_gfx::shaders::heatmap_texture_layer_ubo::HeatmapTexturePropsUBO;
use mln_gfx::shaders::shader_defines::ID_HEATMAP_TEXTURE_PROPS_UBO;
use mln_style::layers::Heatmap;
use mln_util::mat4::{Mat4, matrix};

use super::cast_matrix;
use crate::layer_group::LayerGroup;
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase};
use crate::layer_type::{ErasedEvaluatedLayer, evaluated};
use crate::paint_parameters::PaintParameters;

pub(crate) struct HeatmapTextureLayerTweaker {
    base: LayerTweakerBase,
}

impl HeatmapTextureLayerTweaker {
    pub(crate) fn new(id: String, properties: Arc<dyn ErasedEvaluatedLayer>) -> Self {
        HeatmapTextureLayerTweaker { base: LayerTweakerBase::new(id, Some(properties)) }
    }
}

impl LayerTweaker for HeatmapTextureLayerTweaker {
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

        let Some(properties) = self.base.evaluated_properties.clone() else {
            return;
        };
        let evaluated = &evaluated::<Heatmap>(&properties).paint;

        self.base.properties_updated = false;

        let mut m: Mat4 = [0.0; 16];
        let size = parameters.renderable_size;
        matrix::ortho(&mut m, 0.0, f64::from(size.width), f64::from(size.height), 0.0, -1.0, 1.0);

        let props_ubo = HeatmapTexturePropsUBO {
            matrix: cast_matrix(&m),
            opacity: *evaluated.heatmap_opacity.uniform(),
            pad1: 0.0,
            pad2: 0.0,
            pad3: 0.0,
        };
        layer_group.mutable_uniform_buffers().create_or_update_pod(
            ID_HEATMAP_TEXTURE_PROPS_UBO,
            &props_ubo,
            parameters.context,
        );
        crate::layer_tweaker::bind_no_projection(layer_group.mutable_uniform_buffers(), parameters.context);
    }
}
