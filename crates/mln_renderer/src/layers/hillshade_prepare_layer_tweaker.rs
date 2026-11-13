use std::sync::Arc;

use mln_gfx::LayerGroupBase;
use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shaders::hillshade_prepare_layer_ubo::{HillshadePrepareDrawableUBO, HillshadePrepareTilePropsUBO};
use mln_gfx::shaders::shader_defines::{ID_HILLSHADE_PREPARE_DRAWABLE_UBO, ID_HILLSHADE_PREPARE_TILE_PROPS_UBO};
use mln_style::layer_properties::LayerProperties;
use mln_util::constants::EXTENT;
use mln_util::mat4::{Mat4, matrix};
use mln_util::tileset::RasterEncoding;

use super::cast_matrix;
use crate::geometry::dem_data::get_unpack_vector;
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase};
use crate::paint_parameters::PaintParameters;

#[derive(Clone, Copy, Debug)]
pub struct HillshadePrepareDrawableData {
    pub stride: i32,
    pub encoding: RasterEncoding,
    pub maxzoom: u8,
}

pub struct HillshadePrepareLayerTweaker {
    base: LayerTweakerBase,
}

impl HillshadePrepareLayerTweaker {
    pub fn new(id: String, properties: Arc<dyn LayerProperties>) -> Self {
        HillshadePrepareLayerTweaker { base: LayerTweakerBase::new(id, properties) }
    }
}

impl LayerTweaker for HillshadePrepareLayerTweaker {
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

        self.base.properties_updated = false;

        let self_ptr: *const Self = self;
        visit_layer_group_drawables(layer_group, |drawable| {
            let Some(tile_id) = drawable.get_tile_id() else {
                return;
            };
            let Some(drawable_data) =
                drawable.get_data().and_then(|data| data.downcast_ref::<HillshadePrepareDrawableData>()).copied()
            else {
                return;
            };
            if !super::check_tweak_drawable_ptr(drawable, self_ptr) {
                return;
            }
            let tile_id = tile_id.to_unwrapped();

            let mut m: Mat4 = [0.0; 16];
            let extent = f64::from(EXTENT);
            matrix::ortho(&mut m, 0.0, extent, -extent, 0.0, -1.0, 1.0);
            let a = m;
            matrix::translate(&mut m, &a, 0.0, -extent, 0.0);

            let drawable_ubo = HillshadePrepareDrawableUBO { matrix: cast_matrix(&m) };
            let tile_props_ubo = HillshadePrepareTilePropsUBO {
                unpack: get_unpack_vector(drawable_data.encoding),
                dimension: [drawable_data.stride as f32, drawable_data.stride as f32],
                zoom: f32::from(tile_id.canonical.z),
                maxzoom: f32::from(drawable_data.maxzoom),
            };

            let drawable_uniforms = drawable.mutable_uniform_buffers();
            drawable_uniforms.create_or_update_pod(
                ID_HILLSHADE_PREPARE_DRAWABLE_UBO,
                &drawable_ubo,
                parameters.context,
            );
            drawable_uniforms.create_or_update_pod(
                ID_HILLSHADE_PREPARE_TILE_PROPS_UBO,
                &tile_props_ubo,
                parameters.context,
            );
        });
    }
}
