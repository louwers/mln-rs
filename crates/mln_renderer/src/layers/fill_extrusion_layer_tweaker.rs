// Ports src/mln/renderer/layers/fill_extrusion_layer_tweaker.hpp and
// src/mln/renderer/layers/fill_extrusion_layer_tweaker.cpp
// (MLN_UBO_CONSOLIDATION branches: the port's backend is WebGPU).

use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shaders::fill_extrusion_layer_ubo::{
    FillExtrusionDrawableUBO, FillExtrusionPropsUBO, FillExtrusionTilePropsUBO,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{
    ID_FILL_EXTRUSION_DRAWABLE_UBO, ID_FILL_EXTRUSION_IMAGE_TEXTURE, ID_FILL_EXTRUSION_PROPS_UBO,
    ID_FILL_EXTRUSION_TILE_PROPS_UBO,
};
use mln_gfx::{LayerGroupBase, UniformBufferPtr};
use mln_style::expression::Image;
use mln_style::layer_properties::{LayerProperties, get_crossfade, get_evaluated};
use mln_style::layers::fill_extrusion_layer_properties::{
    FillExtrusionBase, FillExtrusionColor, FillExtrusionHeight, FillExtrusionLayerProperties, FillExtrusionOpacity,
    FillExtrusionPattern, FillExtrusionTranslate, FillExtrusionTranslateAnchor, FillExtrusionVerticalGradient,
};
use mln_style::properties::Property;
use mln_style::renderer::cross_faded_property_evaluator::Faded;
use mln_util::constants::TILE_SIZE_D;
use mln_util::immutable::Immutable;
use mln_util::size::Size;
use mln_util::sync::RwLockExt;

use super::{cast_matrix, check_tweak_drawable_ptr, get_drawable_binders, get_drawable_render_tile, slot_bucket};
use crate::buckets::fill_extrusion_bucket::FillExtrusionBucket;
use crate::frame_projection::zero_to_one_clip_z;
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_projection_data, to_projection_ubo};
use crate::paint_parameters::PaintParameters;

pub struct FillExtrusionLayerTweaker {
    base: LayerTweakerBase,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
    tile_props_uniform_buffer: Option<UniformBufferPtr>,
    props_uniform_buffer: Option<UniformBufferPtr>,
}

impl FillExtrusionLayerTweaker {
    pub fn new(id: String, properties: Immutable<dyn LayerProperties>) -> Self {
        FillExtrusionLayerTweaker {
            base: LayerTweakerBase::new(id, properties),
            drawable_uniform_buffer: None,
            tile_props_uniform_buffer: None,
            props_uniform_buffer: None,
        }
    }
}

/// `util::cast<float>(std::array<uint16_t, 4>)`.
fn cast_tlbr(tlbr: [u16; 4]) -> [f32; 4] {
    tlbr.map(f32::from)
}

impl LayerTweaker for FillExtrusionLayerTweaker {
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
        let evaluated = get_evaluated::<FillExtrusionLayerProperties>(&properties);
        let crossfade = *get_crossfade::<FillExtrusionLayerProperties>(&properties);
        let state = parameters.state;

        // UBO depends on more than just evaluated properties, so we need to
        // update every time, but the resulting buffer can be shared across
        // all the drawables from the layer.
        let props_ubo = FillExtrusionPropsUBO {
            color: evaluated
                .get::<FillExtrusionColor>()
                .constant_or(&FillExtrusionColor::default_value())
                .to_float_array(),
            light_color: FillExtrusionBucket::light_color(&parameters.evaluated_light),
            pad1: 0.0,
            light_position: FillExtrusionBucket::light_position(&parameters.evaluated_light, state),
            base: evaluated.get::<FillExtrusionBase>().constant_or(&FillExtrusionBase::default_value()),
            height: evaluated.get::<FillExtrusionHeight>().constant_or(&FillExtrusionHeight::default_value()),
            light_intensity: FillExtrusionBucket::light_intensity(&parameters.evaluated_light),
            vertical_gradient: if *evaluated.get::<FillExtrusionVerticalGradient>() { 1.0 } else { 0.0 },
            opacity: *evaluated.get::<FillExtrusionOpacity>(),
            fade: crossfade.t,
            from_scale: crossfade.from_scale,
            to_scale: crossfade.to_scale,
            pad2: 0.0,
        };
        context.emplace_or_update_uniform_buffer_pod(&mut self.props_uniform_buffer, &props_ubo);
        layer_group.mutable_uniform_buffers().set(ID_FILL_EXTRUSION_PROPS_UBO, self.props_uniform_buffer.clone());

        self.base.properties_updated = false;

        let zoom = parameters.state.get_zoom() as f32;
        let def_pattern = Faded { from: Image::new(""), to: Image::new("") };
        let fill_pattern_value = evaluated.get::<FillExtrusionPattern>().constant_or(&def_pattern);
        let translation = *evaluated.get::<FillExtrusionTranslate>();
        let anchor = *evaluated.get::<FillExtrusionTranslateAnchor>();

        let mut i: u32 = 0;
        let count = layer_group.get_drawable_count();
        let mut drawable_ubo_vector = vec![FillExtrusionDrawableUBO::default(); count];
        let mut projection_ubo_vector = vec![ProjectionUBO::default(); count];
        let mut tile_props_ubo_vector = vec![FillExtrusionTilePropsUBO::default(); count];

        // OpenGL writes the depth of Metal, Vulkan and WebGPU
        // (`frame_projection`). The globe projects in its shaders.
        let zero_to_one = context.backend_type() == mln_gfx::backend::Type::OpenGL && !state.is_globe_rendering();
        let self_ptr: *const Self = self;
        visit_layer_group_drawables(layer_group, |drawable| {
            const IN_VIEWPORT_PIXEL_UNITS: bool = false; // from RenderTile::translatedMatrix
            const NEAR_CLIPPED: bool = true;
            let Some(tile_id) = drawable.get_tile_id() else {
                return;
            };
            if !check_tweak_drawable_ptr(drawable, self_ptr) {
                return;
            }

            let Some(binders_slot) = get_drawable_binders(drawable) else {
                debug_assert!(false);
                return;
            };
            let Some((bucket, layer_id)) = slot_bucket::<FillExtrusionBucket>(&binders_slot) else {
                debug_assert!(false);
                return;
            };
            let Some(binders_lock) = bucket.paint_property_binders.get(layer_id) else {
                debug_assert!(false);
                return;
            };
            let binders_guard = binders_lock.read_unpoisoned();
            let binders = &*binders_guard;
            let tile = get_drawable_render_tile(drawable);
            if tile.is_none() {
                debug_assert!(false);
                return;
            }

            let tile_id = tile_id.to_unwrapped();
            let mut projection = get_projection_data(
                &tile_id,
                parameters,
                &translation,
                anchor,
                NEAR_CLIPPED,
                IN_VIEWPORT_PIXEL_UNITS,
                drawable,
                false,
            );
            if zero_to_one {
                zero_to_one_clip_z(&mut projection.main_matrix);
                zero_to_one_clip_z(&mut projection.fallback_matrix);
            }
            let matrix = projection.fallback_matrix;
            projection_ubo_vector[i as usize] = to_projection_ubo(&projection);

            let int_zoom = state.get_integer_zoom();
            let tile_ratio = 1.0f32 / tile_id.pixels_to_tile_units(1.0, f32::from(int_zoom));
            let zoom_scale = state.zoom_scale(f64::from(tile_id.canonical.z));
            let nearest_zoom_scale = state.zoom_scale(f64::from(int_zoom) - f64::from(tile_id.canonical.z));
            let tile_size_at_nearest_zoom = (TILE_SIZE_D * nearest_zoom_scale).floor();
            let pixel_x = (tile_size_at_nearest_zoom
                * (f64::from(tile_id.canonical.x) + f64::from(tile_id.wrap) * zoom_scale))
                as i32;
            let pixel_y = (tile_size_at_nearest_zoom * f64::from(tile_id.canonical.y)) as i32;
            let num_tiles = 2f64.powi(i32::from(tile_id.canonical.z));
            let height_factor = (-num_tiles / TILE_SIZE_D / 8.0) as f32;

            let mut texture_size = Size::new(0, 0);
            if let Some(tex) = drawable.get_texture(ID_FILL_EXTRUSION_IMAGE_TEXTURE) {
                texture_size = tex.borrow().get_size();
            }

            let pattern_pos_a = tile.and_then(|t| t.get_pattern(fill_pattern_value.from.id()));
            let pattern_pos_b = tile.and_then(|t| t.get_pattern(fill_pattern_value.to.id()));

            let pattern_t = binders.fill_extrusion_pattern.interpolation_factor(zoom).0;
            drawable_ubo_vector[i as usize] = FillExtrusionDrawableUBO {
                matrix: cast_matrix(&matrix),
                pixel_coord_upper: [(pixel_x >> 16) as f32, (pixel_y >> 16) as f32],
                pixel_coord_lower: [(pixel_x & 0xFFFF) as f32, (pixel_y & 0xFFFF) as f32],
                height_factor,
                tile_ratio,

                base_t: binders.fill_extrusion_base.interpolation_factor(zoom),
                height_t: binders.fill_extrusion_height.interpolation_factor(zoom),
                color_t: binders.fill_extrusion_color.interpolation_factor(zoom),
                pattern_from_t: pattern_t,
                pattern_to_t: pattern_t,
                pad1: 0.0,
            };

            tile_props_ubo_vector[i as usize] = FillExtrusionTilePropsUBO {
                pattern_from: pattern_pos_a.as_ref().map_or([0.0; 4], |p| cast_tlbr(p.tlbr())),
                pattern_to: pattern_pos_b.as_ref().map_or([0.0; 4], |p| cast_tlbr(p.tlbr())),
                texsize: [texture_size.width as f32, texture_size.height as f32],
                pad1: 0.0,
                pad2: 0.0,
            };

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
        layer_uniforms.set(ID_FILL_EXTRUSION_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        layer_uniforms.set(ID_FILL_EXTRUSION_TILE_PROPS_UBO, self.tile_props_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_uniforms, &projection_ubo_vector, context);
    }
}
