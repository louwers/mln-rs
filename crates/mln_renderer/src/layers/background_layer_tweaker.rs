use std::sync::Arc;

use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shaders::background_layer_ubo::{
    BackgroundDrawableUBO, BackgroundDrawableUnionUBO, BackgroundPatternDrawableUBO, BackgroundPatternPropsUBO,
    BackgroundPropsUBO,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{
    ID_BACKGROUND_DRAWABLE_UBO, ID_BACKGROUND_IMAGE_TEXTURE, ID_BACKGROUND_PROPS_UBO,
};
use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_gfx::{LayerGroupBase, UniformBufferPtr};
use mln_style::layer_properties::LayerProperties;
use mln_style::layers::{BackgroundColor, BackgroundOpacity, BackgroundPattern};
use mln_style_types::TranslateAnchorType;
use mln_util::constants::TILE_SIZE_D;

use super::{background_crossfade, background_layer_properties};
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_projection_data, to_projection_ubo};
use crate::paint_parameters::PaintParameters;

pub struct BackgroundLayerTweaker {
    base: LayerTweakerBase,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
}

impl BackgroundLayerTweaker {
    pub fn new(id: String, properties: Arc<dyn LayerProperties>) -> Self {
        BackgroundLayerTweaker { base: LayerTweakerBase::new(id, properties), drawable_uniform_buffer: None }
    }
}

fn cast_matrix(m: &[f64; 16]) -> [f32; 16] {
    std::array::from_fn(|i| m[i] as f32)
}

impl LayerTweaker for BackgroundLayerTweaker {
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
        let evaluated = background_layer_properties(&properties);
        let crossfade = *background_crossfade(&properties);
        let state = parameters.state;
        let has_pattern = !evaluated.get::<BackgroundPattern>().to.empty();
        // conformance: background-pattern.
        let image_pos_a = if has_pattern {
            parameters.pattern_atlas.borrow().get_pattern(evaluated.get::<BackgroundPattern>().from.id())
        } else {
            None
        };
        let image_pos_b = if has_pattern {
            parameters.pattern_atlas.borrow().get_pattern(evaluated.get::<BackgroundPattern>().to.id())
        } else {
            None
        };

        if has_pattern && (image_pos_a.is_none() || image_pos_b.is_none()) {
            // The pattern isn't valid, disable the whole thing.
            layer_group.set_enabled(false);
            return;
        }
        layer_group.set_enabled(true);

        // properties are re-evaluated every time
        self.base.properties_updated = false;

        if let (true, Some(image_pos_a), Some(image_pos_b)) = (has_pattern, &image_pos_a, &image_pos_b) {
            let props_ubo = BackgroundPatternPropsUBO {
                pattern_tl_a: image_pos_a.tl().map(f32::from),
                pattern_br_a: image_pos_a.br().map(f32::from),
                pattern_tl_b: image_pos_b.tl().map(f32::from),
                pattern_br_b: image_pos_b.br().map(f32::from),
                pattern_size_a: image_pos_a.display_size(),
                pattern_size_b: image_pos_b.display_size(),
                scale_a: crossfade.from_scale,
                scale_b: crossfade.to_scale,
                mix: crossfade.t,
                opacity: *evaluated.get::<BackgroundOpacity>(),
            };
            layer_group.mutable_uniform_buffers().create_or_update_pod(ID_BACKGROUND_PROPS_UBO, &props_ubo, context);
        } else {
            let props_ubo = BackgroundPropsUBO {
                color: evaluated.get::<BackgroundColor>().to_float_array(),
                opacity: *evaluated.get::<BackgroundOpacity>(),
                pad1: 0.0,
                pad2: 0.0,
                pad3: 0.0,
            };
            layer_group.mutable_uniform_buffers().create_or_update_pod(ID_BACKGROUND_PROPS_UBO, &props_ubo, context);
        }

        let mut i: u32 = 0;
        let mut drawable_ubo_vector = vec![BackgroundDrawableUnionUBO::default(); layer_group.get_drawable_count()];
        let mut projection_ubo_vector = vec![ProjectionUBO::default(); layer_group.get_drawable_count()];

        let self_ptr: *const Self = self;
        let check = |d: &dyn mln_gfx::Drawable| match d
            .get_layer_tweaker()
            .and_then(|slot| slot.downcast_ref::<crate::layer_tweaker::LayerTweakerPtr>())
        {
            // checkTweakDrawable (the trait method needs `&self`, which the
            // RefCell of the running tweaker already lends out mutably).
            None => true,
            Some(tweaker) => std::ptr::addr_eq(tweaker.as_ptr(), self_ptr),
        };
        visit_layer_group_drawables(layer_group, |drawable| {
            debug_assert!(drawable.get_tile_id().is_some());
            let Some(tile_id) = drawable.get_tile_id() else {
                return;
            };
            if !check(drawable) {
                return;
            }

            // We assume that drawables don't change between pattern and
            // non-pattern.

            let tile_id = tile_id.to_unwrapped();
            let projection = get_projection_data(
                &tile_id,
                parameters,
                &[0.0, 0.0],
                TranslateAnchorType::Viewport,
                false,
                false,
                drawable,
                false,
            );
            let matrix = projection.fallback_matrix;
            projection_ubo_vector[i as usize] = to_projection_ubo(&projection);

            if has_pattern {
                if let Some(tex) = parameters.pattern_atlas.borrow().texture() {
                    tex.borrow_mut().set_sampler_configuration(SamplerState {
                        filter: TextureFilterType::Linear,
                        wrap_u: TextureWrapType::Clamp,
                        wrap_v: TextureWrapType::Clamp,
                        ..Default::default()
                    });
                    drawable.set_texture(Some(tex.clone()), ID_BACKGROUND_IMAGE_TEXTURE);
                }

                let int_zoom = state.get_integer_zoom();
                let tile_size_at_nearest_zoom =
                    (TILE_SIZE_D * state.zoom_scale(f64::from(int_zoom) - f64::from(tile_id.canonical.z))) as i32;
                let pixel_x = (f64::from(tile_size_at_nearest_zoom)
                    * (f64::from(tile_id.canonical.x)
                        + f64::from(tile_id.wrap) * state.zoom_scale(f64::from(tile_id.canonical.z))))
                    as i32;
                let pixel_y = tile_size_at_nearest_zoom.wrapping_mul(tile_id.canonical.y as i32);
                let pix_to_tile = tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom));

                drawable_ubo_vector[i as usize] =
                    BackgroundDrawableUnionUBO::from_pattern_drawable(&BackgroundPatternDrawableUBO {
                        matrix: cast_matrix(&matrix),
                        pixel_coord_upper: [(pixel_x >> 16) as f32, (pixel_y >> 16) as f32],
                        pixel_coord_lower: [(pixel_x & 0xFFFF) as f32, (pixel_y & 0xFFFF) as f32],
                        tile_units_to_pixels: if pix_to_tile == 0.0 { 0.0 } else { 1.0 / pix_to_tile },
                        pad1: 0.0,
                        pad2: 0.0,
                        pad3: 0.0,
                    });
            } else {
                drawable_ubo_vector[i as usize] =
                    BackgroundDrawableUnionUBO::from_drawable(&BackgroundDrawableUBO { matrix: cast_matrix(&matrix) });
            }

            drawable.set_ubo_index(i);
            i += 1;
        });

        let bytes: &[u8] = bytemuck::cast_slice(&drawable_ubo_vector);
        let drawable_ubo_vector_size = bytes.len();
        match &self.drawable_uniform_buffer {
            Some(buffer) if buffer.get_size() >= drawable_ubo_vector_size => buffer.update(bytes),
            _ => {
                self.drawable_uniform_buffer = Some(context.create_uniform_buffer(bytes, false, true));
            }
        }

        layer_group.mutable_uniform_buffers().set(ID_BACKGROUND_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_group.mutable_uniform_buffers(), &projection_ubo_vector, context);
    }
}
