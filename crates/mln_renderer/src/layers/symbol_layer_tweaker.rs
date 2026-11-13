use std::sync::Arc;

use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shaders::shader_defines::{
    ID_SYMBOL_DRAWABLE_UBO, ID_SYMBOL_EVALUATED_PROPS_UBO, ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE,
    ID_SYMBOL_TILE_PROPS_UBO,
};
use mln_gfx::shaders::symbol_layer_ubo::{SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO};
use mln_gfx::{Drawable, LayerGroupBase, UniformBufferPtr};
use mln_map_types::{Projection, ProjectionData};
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::symbol_layer_properties::{
    IconColor, IconHaloBlur, IconHaloColor, IconHaloWidth, IconOpacity, IconTranslate, IconTranslateAnchor,
    SymbolLayerProperties, SymbolScreenSpace, TextColor, TextHaloBlur, TextHaloColor, TextHaloWidth, TextOpacity,
    TextTranslate, TextTranslateAnchor,
};
use mln_style::properties::Property;
use mln_style_types::{AlignmentType, IconTextFitType, SymbolPlacementType};
use mln_util::mat4::{Mat4, matrix};

use super::render_symbol_layer::{SymbolDrawableData, SymbolType, get_screen_matrix};
use super::{cast_matrix, check_tweak_drawable_ptr, get_drawable_binders, get_drawable_render_tile, slot_bucket};
use crate::buckets::symbol_bucket::SymbolBucket;
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_projection_data, to_projection_ubo};
use crate::layout::symbol_projection::{get_gl_coord_matrix, get_label_plane_matrix};
use crate::paint_parameters::PaintParameters;
use crate::render_tile::RenderTile;

pub(crate) struct SymbolLayerTweaker {
    base: LayerTweakerBase,
    evaluated_props_uniform_buffer: Option<UniformBufferPtr>,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
    tile_props_uniform_buffer: Option<UniformBufferPtr>,
}

impl SymbolLayerTweaker {
    pub(crate) fn new(id: String, properties: Arc<dyn LayerProperties>) -> Self {
        SymbolLayerTweaker {
            base: LayerTweakerBase::new(id, properties),
            evaluated_props_uniform_buffer: None,
            drawable_uniform_buffer: None,
            tile_props_uniform_buffer: None,
        }
    }
}

macro_rules! const_or_default {
    ($evaluated:expr, $p:ty) => {
        $evaluated.get::<$p>().constant_or(&<$p>::default_value())
    };
}

fn get_tex_size(drawable: &dyn Drawable, tex_id: usize) -> [f32; 2] {
    match drawable.get_texture(tex_id) {
        Some(tex) => {
            let size = tex.borrow().get_size();
            [size.width as f32, size.height as f32]
        }
        None => [0.0, 0.0],
    }
}

impl LayerTweaker for SymbolLayerTweaker {
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
        let state = parameters.state;
        let properties = self.base.evaluated_properties.clone();
        let symbol_layer_properties =
            (*properties).downcast_ref::<SymbolLayerProperties>().expect("SymbolLayerTweaker: symbol layer properties");
        let evaluated = get_evaluated::<SymbolLayerProperties>(&properties);

        let zoom = state.get_zoom() as f32;
        if self.evaluated_props_uniform_buffer.is_none() || self.base.properties_updated {
            let props_ubo = SymbolEvaluatedPropsUBO {
                text_fill_color: const_or_default!(evaluated, TextColor).to_float_array(),
                text_halo_color: const_or_default!(evaluated, TextHaloColor).to_float_array(),
                text_opacity: const_or_default!(evaluated, TextOpacity),
                text_halo_width: const_or_default!(evaluated, TextHaloWidth),
                text_halo_blur: const_or_default!(evaluated, TextHaloBlur),
                pad1: 0.0,
                icon_fill_color: const_or_default!(evaluated, IconColor).to_float_array(),
                icon_halo_color: const_or_default!(evaluated, IconHaloColor).to_float_array(),
                icon_opacity: const_or_default!(evaluated, IconOpacity),
                icon_halo_width: const_or_default!(evaluated, IconHaloWidth),
                icon_halo_blur: const_or_default!(evaluated, IconHaloBlur),
                pad2: 0.0,
            };
            context.emplace_or_update_uniform_buffer_pod(&mut self.evaluated_props_uniform_buffer, &props_ubo);
            self.base.properties_updated = false;
        }
        layer_group
            .mutable_uniform_buffers()
            .set(ID_SYMBOL_EVALUATED_PROPS_UBO, self.evaluated_props_uniform_buffer.clone());

        // Only enabled drawables draw, so only they get an entry.
        let count = layer_group.get_drawable_count();
        let mut drawable_ubo_vector = Vec::with_capacity(count);
        let mut tile_props_ubo_vector = Vec::with_capacity(count);
        let mut projection_ubo_vector = Vec::with_capacity(count);

        let cam_dist = state.get_camera_to_center_distance();
        let screen_space_prop = symbol_layer_properties.layer_impl().layout.get::<SymbolScreenSpace>();
        let is_screen_space = if screen_space_prop.is_constant() {
            *screen_space_prop.as_constant()
        } else {
            SymbolScreenSpace::default_value()
        };

        let pitched_scale = state.get_projection().circle_radius_correction(state) as f32;

        let self_ptr: *const Self = self;
        let id = self.base.id.clone();
        visit_layer_group_drawables(layer_group, |drawable| {
            if !drawable.get_enabled() {
                return;
            }
            let Some(tile_id) = drawable.get_tile_id() else {
                return;
            };
            let Some(symbol_data) = drawable.get_data().and_then(|d| d.downcast_ref::<SymbolDrawableData>()).copied()
            else {
                return;
            };
            if !check_tweak_drawable_ptr(drawable, self_ptr) {
                return;
            }
            let tile_id = tile_id.to_unwrapped();
            let is_text = symbol_data.symbol_type == SymbolType::Text;

            let Some(binders_slot) = get_drawable_binders(drawable) else {
                debug_assert!(false);
                return;
            };
            let Some((bucket, _layer_id)) = slot_bucket::<SymbolBucket>(&binders_slot) else {
                debug_assert!(false);
                return;
            };
            if get_drawable_render_tile(drawable).is_none() {
                debug_assert!(false);
                return;
            }
            let Some(paint_properties) = bucket.paint_properties.get(&id) else {
                debug_assert!(false);
                return;
            };
            let paint_properties = paint_properties.read();

            let translate = if is_text { evaluated.get::<TextTranslate>() } else { evaluated.get::<IconTranslate>() };
            let anchor =
                if is_text { *evaluated.get::<TextTranslateAnchor>() } else { *evaluated.get::<IconTranslateAnchor>() };
            let mut projection: ProjectionData = if is_screen_space {
                Projection::Mercator.get_projection_data(state, &tile_id, &get_screen_matrix(*translate))
            } else {
                const NEAR_CLIPPED: bool = false;
                const IN_VIEWPORT_PIXEL_UNITS: bool = false;
                get_projection_data(
                    &tile_id,
                    parameters,
                    [0.0, 0.0],
                    anchor,
                    NEAR_CLIPPED,
                    IN_VIEWPORT_PIXEL_UNITS,
                    drawable,
                    false,
                )
            };
            // The translation stays out of the matrices on every projection:
            // the shader adds it to point anchors, and placement to the
            // positions it lays out (variable anchors, line labels), so it
            // applies once.
            let tile_translation: [f32; 2] = if is_screen_space {
                [0.0, 0.0]
            } else {
                RenderTile::tile_unit_translation(&tile_id, *translate, anchor, state)
            };
            projection.translate = [0.0, 0.0];
            let matrix = projection.fallback_matrix;
            projection_ubo_vector.push(to_projection_ubo(&projection));

            // from symbol_program, makeValues
            let current_zoom = state.get_zoom() as f32;
            let pixels_to_tile_units = tile_id.pixels_to_tile_units(1.0, current_zoom);
            let pitch_with_map = symbol_data.pitch_alignment == AlignmentType::Map;
            let rotate_with_map = symbol_data.rotation_alignment == AlignmentType::Map;
            let along_line = symbol_data.placement != SymbolPlacementType::Point
                && symbol_data.rotation_alignment == AlignmentType::Map;
            let has_variable_placement =
                symbol_data.bucket_variable_placement && (is_text || symbol_data.text_fit != IconTextFitType::None);
            let label_plane_matrix = if along_line || has_variable_placement {
                let mut m: Mat4 = [0.0; 16];
                matrix::identity(&mut m);
                m
            } else {
                get_label_plane_matrix(pitch_with_map, rotate_with_map, state, pixels_to_tile_units)
            };
            let gl_coord_matrix = get_gl_coord_matrix(pitch_with_map, rotate_with_map, state, pixels_to_tile_units);
            let gamma_scale = if symbol_data.pitch_alignment == AlignmentType::Map {
                (state.get_pitch().cos() as f32) * cam_dist
            } else {
                1.0
            };

            // Line label rotation happens in
            // `updateLineLabels`/`reprojectLineLabels`. Pitched point labels
            // are automatically rotated by the labelPlaneMatrix projection.
            // Unpitched point labels need to have their rotation applied
            // after projection
            let rotate_in_shader = rotate_with_map && !pitch_with_map && !along_line;

            let size_binder = if is_text { &bucket.text_size_binder } else { &bucket.icon_size_binder };
            let size = size_binder.evaluate_for_zoom(current_zoom);

            let (fill_color_t, halo_color_t, opacity_t, halo_width_t, halo_blur_t) = if is_text {
                let b = &paint_properties.text_binders;
                (
                    b.text_color.interpolation_factor(zoom),
                    b.text_halo_color.interpolation_factor(zoom),
                    b.text_opacity.interpolation_factor(zoom),
                    b.text_halo_width.interpolation_factor(zoom),
                    b.text_halo_blur.interpolation_factor(zoom),
                )
            } else {
                let b = &paint_properties.icon_binders;
                (
                    b.icon_color.interpolation_factor(zoom),
                    b.icon_halo_color.interpolation_factor(zoom),
                    b.icon_opacity.interpolation_factor(zoom),
                    b.icon_halo_width.interpolation_factor(zoom),
                    b.icon_halo_blur.interpolation_factor(zoom),
                )
            };

            drawable_ubo_vector.push(SymbolDrawableUBO {
                matrix: cast_matrix(&matrix),
                label_plane_matrix: cast_matrix(&label_plane_matrix),
                coord_matrix: cast_matrix(&gl_coord_matrix),
                texsize: get_tex_size(drawable, ID_SYMBOL_IMAGE_TEXTURE),
                texsize_icon: get_tex_size(drawable, ID_SYMBOL_IMAGE_ICON_TEXTURE),
                is_text_prop: i32::from(is_text),
                rotate_symbol: i32::from(rotate_in_shader),
                pitch_with_map: i32::from(pitch_with_map),
                is_size_zoom_constant: i32::from(size.is_zoom_constant),
                is_size_feature_constant: i32::from(size.is_feature_constant),
                is_offset: i32::from(symbol_data.is_offset),
                size_t: size.size_t,
                size: size.size,
                fill_color_t,
                halo_color_t,
                opacity_t,
                halo_width_t,
                halo_blur_t,
                is_along_line: i32::from(along_line),
                is_variable_anchor: i32::from(has_variable_placement),
                pitched_scale,
                translation: tile_translation,
                pad1: 0.0,
                pad2: 0.0,
            });
            tile_props_ubo_vector.push(SymbolTilePropsUBO {
                is_text: i32::from(is_text),
                is_halo: i32::from(symbol_data.is_halo),
                gamma_scale,
                pad1: 0.0,
            });
            drawable.set_ubo_index((drawable_ubo_vector.len() - 1) as u32);
        });
        if drawable_ubo_vector.is_empty() {
            return;
        }

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
        layer_uniforms.set(ID_SYMBOL_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        layer_uniforms.set(ID_SYMBOL_TILE_PROPS_UBO, self.tile_props_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_uniforms, &projection_ubo_vector, context);
    }
}
