// Ports src/mln/renderer/layers/collision_layer_tweaker.hpp,
// src/mln/renderer/layers/collision_layer_tweaker.cpp and
// src/mln/gfx/collision_drawable_data.hpp.

use mln_gfx::LayerGroupBase;
use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shaders::shader_defines::{
    ID_COLLISION_DRAWABLE_UBO, ID_COLLISION_PROJECTION_UBO, ID_COLLISION_TILE_PROPS_UBO,
};
use mln_gfx::shaders::symbol_layer_ubo::{CollisionDrawableUBO, CollisionTilePropsUBO};
use mln_style::layer_properties::LayerProperties;
use mln_style_types::TranslateAnchorType;
use mln_util::immutable::Immutable;

use super::{cast_matrix, check_tweak_drawable_ptr};
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_projection_data, to_projection_ubo};
use crate::paint_parameters::PaintParameters;

/// `gfx::CollisionDrawableData`.
#[derive(Clone, Copy, Debug)]
pub struct CollisionDrawableData {
    pub translate: [f32; 2],
    pub translate_anchor: TranslateAnchorType,
}

pub struct CollisionLayerTweaker {
    base: LayerTweakerBase,
}

impl CollisionLayerTweaker {
    pub fn new(id: String, properties: Immutable<dyn LayerProperties>) -> Self {
        CollisionLayerTweaker { base: LayerTweakerBase::new(id, properties) }
    }
}

impl LayerTweaker for CollisionLayerTweaker {
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
        self.base.properties_updated = false;

        let self_ptr: *const Self = self;
        visit_layer_group_drawables(layer_group, |drawable| {
            const NEAR_CLIPPED: bool = false;
            const IN_VIEWPORT_PIXEL_UNITS: bool = false;
            let Some(overscaled_tile_id) = drawable.get_tile_id() else {
                return;
            };
            let Some(data) = drawable.get_data().and_then(|d| d.downcast_ref::<CollisionDrawableData>()).copied()
            else {
                return;
            };
            if !check_tweak_drawable_ptr(drawable, self_ptr) {
                return;
            }
            let tile_id = overscaled_tile_id.to_unwrapped();

            // matrix
            let projection = get_projection_data(
                &tile_id,
                parameters,
                &data.translate,
                data.translate_anchor,
                NEAR_CLIPPED,
                IN_VIEWPORT_PIXEL_UNITS,
                drawable,
                false,
            );
            let matrix = projection.fallback_matrix;
            let projection_ubo = to_projection_ubo(&projection);

            // extrude scale
            let zoom = parameters.state.get_zoom();
            let pixel_ratio = tile_id.pixels_to_tile_units(1.0, zoom as f32);
            let scale = 2f64.powf(zoom - f64::from(overscaled_tile_id.overscaled_z)) as f32;
            let extrude_scale = [
                parameters.pixels_to_gl_units[0] / (pixel_ratio * scale),
                parameters.pixels_to_gl_units[1] / (pixel_ratio * scale),
            ];
            let drawable_ubo = CollisionDrawableUBO { matrix: cast_matrix(&matrix) };
            let tile_props_ubo = CollisionTilePropsUBO {
                extrude_scale,
                overscale_factor: overscaled_tile_id.overscale_factor() as f32,
                pad1: 0.0,
            };
            let drawable_uniforms = drawable.mutable_uniform_buffers();
            drawable_uniforms.create_or_update_pod(ID_COLLISION_DRAWABLE_UBO, &drawable_ubo, context);
            drawable_uniforms.create_or_update_pod(ID_COLLISION_TILE_PROPS_UBO, &tile_props_ubo, context);
            drawable_uniforms.create_or_update_pod(ID_COLLISION_PROJECTION_UBO, &projection_ubo, context);
        });
    }
}
