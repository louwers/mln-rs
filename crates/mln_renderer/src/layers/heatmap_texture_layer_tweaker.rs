// Ports src/mln/renderer/layers/heatmap_texture_layer_tweaker.hpp and
// src/mln/renderer/layers/heatmap_texture_layer_tweaker.cpp.

use mln_gfx::LayerGroupBase;
use mln_gfx::shaders::heatmap_texture_layer_ubo::HeatmapTexturePropsUBO;
use mln_gfx::shaders::shader_defines::ID_HEATMAP_TEXTURE_PROPS_UBO;
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::heatmap_layer_properties::{HeatmapLayerProperties, HeatmapOpacity};
use mln_util::immutable::Immutable;
use mln_util::mat4::{Mat4, matrix};

use super::cast_matrix;
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase};
use crate::paint_parameters::PaintParameters;

pub struct HeatmapTextureLayerTweaker {
    base: LayerTweakerBase,
}

impl HeatmapTextureLayerTweaker {
    pub fn new(id: String, properties: Immutable<dyn LayerProperties>) -> Self {
        HeatmapTextureLayerTweaker { base: LayerTweakerBase::new(id, properties) }
    }
}

impl LayerTweaker for HeatmapTextureLayerTweaker {
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

        let properties = self.base.evaluated_properties.clone();
        let evaluated = get_evaluated::<HeatmapLayerProperties>(&properties);

        self.base.properties_updated = false;

        let mut m: Mat4 = [0.0; 16];
        let size = parameters.renderable_size;
        matrix::ortho(&mut m, 0.0, f64::from(size.width), f64::from(size.height), 0.0, -1.0, 1.0);

        let props_ubo = HeatmapTexturePropsUBO {
            matrix: cast_matrix(&m),
            opacity: *evaluated.get::<HeatmapOpacity>(),
            pad1: 0.0,
            pad2: 0.0,
            pad3: 0.0,
        };
        layer_group.mutable_uniform_buffers().create_or_update_pod(
            ID_HEATMAP_TEXTURE_PROPS_UBO,
            &props_ubo,
            parameters.context,
        );
    }
}
