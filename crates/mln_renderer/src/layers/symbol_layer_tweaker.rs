use std::sync::Arc;

use mln_gfx::shaders::shader_defines::{
    ID_SYMBOL_DRAWABLE_UBO, ID_SYMBOL_EVALUATED_PROPS_UBO, ID_SYMBOL_IMAGE_ICON_TEXTURE, ID_SYMBOL_IMAGE_TEXTURE,
    ID_SYMBOL_TILE_PROPS_UBO,
};
use mln_gfx::shaders::symbol_layer_ubo::{SymbolDrawableUBO, SymbolEvaluatedPropsUBO, SymbolTilePropsUBO};
use mln_map_types::{Projection, ProjectionData};
use mln_style::layers::{Symbol, SymbolLayoutProperties};
use mln_style_types::{AlignmentType, IconTextFitType};
use mln_util::mat4::{Mat4, matrix};

use super::render_symbol_layer::{SymbolType, get_screen_matrix};
use super::{LayerTiles, cast_matrix};
use crate::drawable::Drawable;
use crate::gfx::uniform_buffer::UniformBufferPtr;
use crate::layer_group::LayerGroup;
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_projection_data, to_projection_ubo};
use crate::layer_type::{ErasedEvaluatedLayer, evaluated};
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
    pub(crate) fn new(id: String, properties: Arc<dyn ErasedEvaluatedLayer>) -> Self {
        SymbolLayerTweaker {
            base: LayerTweakerBase::new(id, Some(properties)),
            evaluated_props_uniform_buffer: None,
            drawable_uniform_buffer: None,
            tile_props_uniform_buffer: None,
        }
    }
}

fn get_tex_size(drawable: &Drawable, tex_id: usize) -> [f32; 2] {
    match drawable.objects.textures[tex_id].as_ref() {
        Some(tex) => {
            let size = tex.borrow().size();
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

    fn execute(&mut self, layer_group: &mut LayerGroup, tiles: &LayerTiles<'_>, parameters: &PaintParameters<'_>) {
        if layer_group.empty() {
            return;
        }

        let context = parameters.context;
        let state = parameters.state;
        let Some(properties) = self.base.evaluated_properties.clone() else {
            return;
        };
        let layer = evaluated::<Symbol>(&properties);
        let evaluated = &layer.paint;

        let zoom = state.get_zoom() as f32;
        if self.evaluated_props_uniform_buffer.is_none() || self.base.properties_updated {
            let props_ubo = SymbolEvaluatedPropsUBO {
                text_fill_color: evaluated.text_color.uniform().to_float_array(),
                text_halo_color: evaluated.text_halo_color.uniform().to_float_array(),
                text_opacity: *evaluated.text_opacity.uniform(),
                text_halo_width: *evaluated.text_halo_width.uniform(),
                text_halo_blur: *evaluated.text_halo_blur.uniform(),
                pad1: 0.0,
                icon_fill_color: evaluated.icon_color.uniform().to_float_array(),
                icon_halo_color: evaluated.icon_halo_color.uniform().to_float_array(),
                icon_opacity: *evaluated.icon_opacity.uniform(),
                icon_halo_width: *evaluated.icon_halo_width.uniform(),
                icon_halo_blur: *evaluated.icon_halo_blur.uniform(),
                pad2: 0.0,
            };
            context.write_uniform_buffer_pod(&mut self.evaluated_props_uniform_buffer, &props_ubo);
            self.base.properties_updated = false;
        }
        layer_group
            .mutable_uniform_buffers()
            .set(ID_SYMBOL_EVALUATED_PROPS_UBO, self.evaluated_props_uniform_buffer.clone());

        // Only enabled drawables draw, so only they get an entry.
        let count = layer_group.drawable_count();
        let mut drawable_ubo_vector = Vec::with_capacity(count);
        let mut tile_props_ubo_vector = Vec::with_capacity(count);
        let mut projection_ubo_vector = Vec::with_capacity(count);

        let cam_dist = state.get_camera_to_center_distance();
        let screen_space_prop = &layer.style.layout.symbol_screen_space;
        let is_screen_space = if screen_space_prop.is_constant() {
            *screen_space_prop.as_constant()
        } else {
            SymbolLayoutProperties::Defaults::symbol_screen_space()
        };

        let pitched_scale = state.get_projection().circle_radius_correction(state) as f32;

        let key = self.base.key;
        let id = self.base.id.clone();
        layer_group.visit_drawables(|drawable| {
            if !drawable.enabled {
                return;
            }
            let Some(tile_id) = drawable.tile_id else {
                return;
            };
            let Some(crate::drawable::DrawableData::Symbol(symbol_data)) = &drawable.data else {
                return;
            };
            let symbol_data = *symbol_data;
            if !drawable.tweaked_by(key) {
                return;
            }
            let Some((_, bucket)) = tiles.bucket::<Symbol>(&tile_id) else {
                debug_assert!(false);
                return;
            };
            let tile_id = tile_id.to_unwrapped();
            let is_text = symbol_data.symbol_type == SymbolType::Text;

            let Some(paint_properties) = bucket.paint_properties.get(&id) else {
                debug_assert!(false);
                return;
            };
            let paint_properties = paint_properties.read();

            let translate = if is_text { &evaluated.text_translate } else { &evaluated.icon_translate };
            let anchor = if is_text { &evaluated.text_translate_anchor } else { &evaluated.icon_translate_anchor };
            let mut projection: ProjectionData = if is_screen_space {
                Projection::Mercator.get_projection_data(state, &tile_id, &get_screen_matrix(*(*translate).uniform()))
            } else {
                const NEAR_CLIPPED: bool = false;
                const IN_VIEWPORT_PIXEL_UNITS: bool = false;
                get_projection_data(
                    &tile_id,
                    parameters,
                    [0.0, 0.0],
                    *anchor.uniform(),
                    NEAR_CLIPPED,
                    IN_VIEWPORT_PIXEL_UNITS,
                    drawable.origin,
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
                RenderTile::tile_unit_translation(&tile_id, *(*translate).uniform(), *anchor.uniform(), state)
            };
            projection.translate = [0.0, 0.0];
            let matrix = projection.fallback_matrix;
            projection_ubo_vector.push(to_projection_ubo(&projection));

            // from symbol_program, makeValues
            let current_zoom = state.get_zoom() as f32;
            let pixels_to_tile_units = tile_id.pixels_to_tile_units(1.0, current_zoom);
            let pitch_with_map = symbol_data.pitch_alignment == AlignmentType::Map;
            let rotate_with_map = symbol_data.rotation_alignment == AlignmentType::Map;
            let along_line = symbol_data.along_line;
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
            drawable.ubo_index = (drawable_ubo_vector.len() - 1) as u32;
        });
        if drawable_ubo_vector.is_empty() {
            return;
        }

        let bytes: &[u8] = bytemuck::cast_slice(&drawable_ubo_vector);
        context.write_storage_buffer(&mut self.drawable_uniform_buffer, bytes);
        let bytes: &[u8] = bytemuck::cast_slice(&tile_props_ubo_vector);
        context.write_storage_buffer(&mut self.tile_props_uniform_buffer, bytes);

        let layer_uniforms = layer_group.mutable_uniform_buffers();
        layer_uniforms.set(ID_SYMBOL_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        layer_uniforms.set(ID_SYMBOL_TILE_PROPS_UBO, self.tile_props_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_uniforms, &projection_ubo_vector, context);
    }
}
