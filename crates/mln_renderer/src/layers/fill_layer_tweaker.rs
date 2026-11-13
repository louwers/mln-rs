use std::sync::Arc;

use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shaders::fill_layer_ubo::{
    FillDrawableUBO, FillDrawableUnionUBO, FillEvaluatedPropsUBO, FillOutlineDrawableUBO,
    FillOutlineTriangulatedDrawableUBO, FillPatternDrawableUBO, FillPatternTilePropsUBO, FillTilePropsUnionUBO,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{
    ID_FILL_DRAWABLE_UBO, ID_FILL_EVALUATED_PROPS_UBO, ID_FILL_IMAGE_TEXTURE, ID_FILL_TILE_PROPS_UBO,
};
use mln_gfx::{LayerGroupBase, UniformBufferPtr};
use mln_style::expression::Image;
use mln_style::layer_properties::{LayerProperties, get_crossfade, get_evaluated};
use mln_style::layers::fill_layer_properties::{
    FillColor, FillLayerProperties, FillOpacity, FillOutlineColor, FillPattern, FillTranslate, FillTranslateAnchor,
};
use mln_style::properties::Property;
use mln_style::renderer::cross_faded_property_evaluator::Faded;
use mln_util::constants::TILE_SIZE_D;
use mln_util::logging::{Event, Log};
use mln_util::size::Size;

use super::render_fill_layer::FillVariant;
use super::{cast_matrix, check_tweak_drawable_ptr, get_drawable_binders, get_drawable_render_tile, slot_bucket};
use crate::bucket::Bucket;
use crate::buckets::fill_bucket::FillBucket;
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_projection_data, to_projection_ubo};
use crate::paint_parameters::PaintParameters;

pub(crate) struct FillLayerTweaker {
    base: LayerTweakerBase,
    evaluated_props_uniform_buffer: Option<UniformBufferPtr>,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
    tile_props_uniform_buffer: Option<UniformBufferPtr>,
}

impl FillLayerTweaker {
    pub(crate) fn new(id: String, properties: Arc<dyn LayerProperties>) -> Self {
        FillLayerTweaker {
            base: LayerTweakerBase::new(id, properties),
            evaluated_props_uniform_buffer: None,
            drawable_uniform_buffer: None,
            tile_props_uniform_buffer: None,
        }
    }
}

fn cast_tlbr(a: [u16; 4]) -> [f32; 4] {
    [f32::from(a[0]), f32::from(a[1]), f32::from(a[2]), f32::from(a[3])]
}

impl LayerTweaker for FillLayerTweaker {
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
        let properties = Arc::clone(&self.base.evaluated_properties);
        let evaluated = get_evaluated::<FillLayerProperties>(&properties);
        let crossfade = *get_crossfade::<FillLayerProperties>(&properties);

        if self.evaluated_props_uniform_buffer.is_none() || self.base.properties_updated {
            let props_ubo = FillEvaluatedPropsUBO {
                color: evaluated.get::<FillColor>().constant_or(&FillColor::default_value()).to_float_array(),
                outline_color: evaluated
                    .get::<FillOutlineColor>()
                    .constant_or(&FillOutlineColor::default_value())
                    .to_float_array(),
                opacity: evaluated.get::<FillOpacity>().constant_or(&FillOpacity::default_value()),
                fade: crossfade.t,
                from_scale: crossfade.from_scale,
                to_scale: crossfade.to_scale,
            };
            context.emplace_or_update_uniform_buffer_pod(&mut self.evaluated_props_uniform_buffer, &props_ubo);
            self.base.properties_updated = false;
        }
        layer_group
            .mutable_uniform_buffers()
            .set(ID_FILL_EVALUATED_PROPS_UBO, self.evaluated_props_uniform_buffer.clone());

        let translation = *evaluated.get::<FillTranslate>();
        let anchor = *evaluated.get::<FillTranslateAnchor>();
        let zoom = parameters.state.get_zoom() as f32;
        let int_zoom = parameters.state.get_integer_zoom();

        let mut i: u32 = 0;
        let count = layer_group.get_drawable_count();
        let mut drawable_ubo_vector = vec![FillDrawableUnionUBO::default(); count];
        let mut projection_ubo_vector = vec![ProjectionUBO::default(); count];
        let mut tile_props_ubo_vector = vec![FillTilePropsUnionUBO::default(); count];

        let self_ptr: *const Self = self;
        let id = self.base.id.clone();
        visit_layer_group_drawables(layer_group, |drawable| {
            const IN_VIEWPORT_PIXEL_UNITS: bool = false; // from RenderTile::translatedMatrix
            const NEAR_CLIPPED: bool = false;
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
            let Some((bucket, layer_id)) = slot_bucket::<FillBucket>(&binders_slot) else {
                debug_assert!(false);
                return;
            };
            let Some(binders_lock) = bucket.paint_property_binders.get(layer_id) else {
                debug_assert!(false);
                return;
            };
            let binders_guard = binders_lock.read();
            let binders = &*binders_guard;
            let tile = get_drawable_render_tile(drawable);
            debug_assert!(tile.is_some());

            let sdf_pattern: f32 = if bucket.is_sdf_pattern(&id) { 1.0 } else { 0.0 };

            let fill_pattern_value =
                evaluated.get::<FillPattern>().constant_or(&Faded { from: Image::new(""), to: Image::new("") });
            let pattern_pos_a = tile.and_then(|t| t.get_pattern(fill_pattern_value.from.id()));
            let pattern_pos_b = tile.and_then(|t| t.get_pattern(fill_pattern_value.to.id()));

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

            // from FillPatternProgram::layoutUniformValues
            let tile_ratio = 1.0f32 / tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom));
            let tile_size_at_nearest_zoom = (TILE_SIZE_D
                * parameters.state.zoom_scale(f64::from(int_zoom) - f64::from(tile_id.canonical.z)))
                as i32;
            let pixel_x = (f64::from(tile_size_at_nearest_zoom)
                * (f64::from(tile_id.canonical.x)
                    + f64::from(tile_id.wrap) * parameters.state.zoom_scale(f64::from(tile_id.canonical.z))))
                as i32;
            let pixel_y = tile_size_at_nearest_zoom.wrapping_mul(tile_id.canonical.y as i32);

            let mut texture_size = Size::new(0, 0);
            if let Some(tex) = drawable.get_texture(ID_FILL_IMAGE_TEXTURE) {
                texture_size = tex.borrow().get_size();
            }

            let ubo = &mut drawable_ubo_vector[i as usize];
            match FillVariant::from_type(drawable.get_type()) {
                Some(FillVariant::Fill) => {
                    ubo.set_fill(&FillDrawableUBO {
                        matrix: cast_matrix(&matrix),

                        color_t: binders.fill_color.interpolation_factor(zoom),
                        opacity_t: binders.fill_opacity.interpolation_factor(zoom),
                        pad1: 0.0,
                        pad2: 0.0,
                    });
                }
                Some(FillVariant::FillOutline) => {
                    ubo.set_fill_outline(&FillOutlineDrawableUBO {
                        matrix: cast_matrix(&matrix),

                        outline_color_t: binders.fill_outline_color.interpolation_factor(zoom),
                        opacity_t: binders.fill_opacity.interpolation_factor(zoom),
                        pad1: 0.0,
                        pad2: 0.0,
                    });
                }
                Some(FillVariant::FillPattern | FillVariant::FillOutlinePattern) => {
                    ubo.set_fill_pattern(&FillPatternDrawableUBO {
                        matrix: cast_matrix(&matrix),
                        pixel_coord_upper: [(pixel_x >> 16) as f32, (pixel_y >> 16) as f32],
                        pixel_coord_lower: [(pixel_x & 0xFFFF) as f32, (pixel_y & 0xFFFF) as f32],
                        tile_ratio,

                        pattern_from_t: binders.fill_pattern.interpolation_factor(zoom).0,
                        pattern_to_t: binders.fill_pattern.interpolation_factor(zoom).0,
                        opacity_t: binders.fill_opacity.interpolation_factor(zoom),
                        color_t: binders.fill_color.interpolation_factor(zoom),
                        pad1: 0.0,
                        pad2: 0.0,
                        pad3: 0.0,
                    });
                    tile_props_ubo_vector[i as usize].fill_pattern_tile_props_ubo = FillPatternTilePropsUBO {
                        pattern_from: pattern_pos_a.as_ref().map_or([0.0; 4], |p| cast_tlbr(p.tlbr())),
                        pattern_to: pattern_pos_b.as_ref().map_or([0.0; 4], |p| cast_tlbr(p.tlbr())),
                        texsize: [texture_size.width as f32, texture_size.height as f32],
                        sdf: sdf_pattern,
                        pad2: 0.0,
                    };
                }
                Some(FillVariant::FillOutlineTriangulated) => {
                    ubo.set_fill_outline_triangulated(&FillOutlineTriangulatedDrawableUBO {
                        matrix: cast_matrix(&matrix),
                        ratio: 1.0f32 / tile_id.pixels_to_tile_units(1.0, parameters.state.get_zoom() as f32),
                        pad1: 0.0,
                        pad2: 0.0,
                        pad3: 0.0,
                    });
                }
                None => {
                    if cfg!(debug_assertions) {
                        Log::error(Event::Render, "Invalid fill variant type supplied during drawable update!");
                    }
                }
            }
            drawable.set_ubo_index(i);
            i += 1;
        });

        let bytes: &[u8] = bytemuck::cast_slice(&drawable_ubo_vector);
        match &self.drawable_uniform_buffer {
            Some(buffer) if buffer.get_size() >= bytes.len() => buffer.update(bytes),
            _ => self.drawable_uniform_buffer = Some(context.create_uniform_buffer(bytes, false, true)),
        }

        let bytes: &[u8] = bytemuck::cast_slice(&tile_props_ubo_vector);
        match &self.tile_props_uniform_buffer {
            Some(buffer) if buffer.get_size() >= bytes.len() => buffer.update(bytes),
            _ => self.tile_props_uniform_buffer = Some(context.create_uniform_buffer(bytes, false, true)),
        }

        let layer_uniforms = layer_group.mutable_uniform_buffers();
        layer_uniforms.set(ID_FILL_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        layer_uniforms.set(ID_FILL_TILE_PROPS_UBO, self.tile_props_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_uniforms, &projection_ubo_vector, context);
    }
}
