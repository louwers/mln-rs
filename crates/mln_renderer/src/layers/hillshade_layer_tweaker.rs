use std::sync::Arc;

use mln_gfx::renderer::layer_group::visit_layer_group_drawables;
use mln_gfx::shaders::hillshade_layer_ubo::{
    HillshadeDrawableUBO, HillshadeEvaluatedPropsUBO, HillshadeTilePropsUBO, MAX_ILLUMINATION_SOURCES,
};
use mln_gfx::shaders::layer_ubo::ProjectionUBO;
use mln_gfx::shaders::shader_defines::{
    ID_HILLSHADE_DRAWABLE_UBO, ID_HILLSHADE_EVALUATED_PROPS_UBO, ID_HILLSHADE_TILE_PROPS_UBO,
};
use mln_gfx::{LayerGroupBase, UniformBufferPtr};
use mln_math::deg2radf;
use mln_style::layer_properties::{LayerProperties, get_evaluated};
use mln_style::layers::hillshade_layer_properties::{
    HillshadeAccentColor, HillshadeExaggeration, HillshadeHighlightColor, HillshadeIlluminationAltitude,
    HillshadeIlluminationAnchor, HillshadeIlluminationDirection, HillshadeLayerProperties, HillshadeMethod,
    HillshadePaintProperties, HillshadeShadowColor,
};
use mln_style_types::{HillshadeIlluminationAnchorType, HillshadeMethodType, TranslateAnchorType};
use mln_tile_id::UnwrappedTileID;
use mln_util::Color;
use mln_util::geo::LatLng;

use super::{cast_matrix, check_tweak_drawable_ptr};
use crate::layer_tweaker::{LayerTweaker, LayerTweakerBase, get_projection_data, to_projection_ubo};
use crate::paint_parameters::PaintParameters;

/// Structure to hold processed illumination properties
struct IlluminationProperties {
    direction_radians: Vec<f32>,
    altitude_radians: Vec<f32>,
    shadow_colors: Vec<Color>,
    highlight_colors: Vec<Color>,
}

impl IlluminationProperties {
    fn num_sources(&self) -> usize {
        self.direction_radians.len()
    }
}

/// Pad shorter arrays by repeating the last element.
fn pad_array<T: Clone + Default>(arr: &mut Vec<T>, max_length: usize) {
    if arr.is_empty() {
        arr.push(T::default());
    }
    while arr.len() < max_length {
        let last = arr.last().expect("non-empty").clone();
        arr.push(last);
    }
}

/// Convert style properties to illumination properties
fn get_illumination_properties(evaluated: &HillshadePaintProperties::PossiblyEvaluated) -> IlluminationProperties {
    // Get the values from evaluated properties (these are now vectors)
    let mut directions: Vec<f32> = evaluated.get::<HillshadeIlluminationDirection>().clone();
    let mut altitudes: Vec<f32> = evaluated.get::<HillshadeIlluminationAltitude>().clone();
    let mut highlights: Vec<Color> = evaluated.get::<HillshadeHighlightColor>().clone();
    let mut shadows: Vec<Color> = evaluated.get::<HillshadeShadowColor>().clone();

    // Find the maximum length to ensure all arrays are the same size
    let mut max_length = directions.len().max(altitudes.len()).max(highlights.len()).max(shadows.len());

    // Ensure we don't exceed the maximum supported
    max_length = max_length.min(MAX_ILLUMINATION_SOURCES);

    pad_array(&mut directions, max_length);
    pad_array(&mut altitudes, max_length);
    pad_array(&mut highlights, max_length);
    pad_array(&mut shadows, max_length);

    // Convert degrees to radians
    IlluminationProperties {
        direction_radians: directions.iter().map(|&deg| deg2radf(deg)).collect(),
        altitude_radians: altitudes.iter().map(|&deg| deg2radf(deg)).collect(),
        shadow_colors: shadows,
        highlight_colors: highlights,
    }
}

/// Pack illumination properties into the UBO format
fn pack_evaluated_props(illumination: &IlluminationProperties, accent_color: &Color) -> HillshadeEvaluatedPropsUBO {
    let mut ubo = HillshadeEvaluatedPropsUBO {
        // Pack accent color
        accent: [accent_color.r, accent_color.g, accent_color.b, accent_color.a],
        ..HillshadeEvaluatedPropsUBO::default()
    };

    // Pack altitudes (up to 4)
    for (i, &altitude) in illumination.altitude_radians.iter().take(4).enumerate() {
        ubo.altitudes[i] = altitude;
    }

    // Pack azimuths (up to 4)
    for (i, &azimuth) in illumination.direction_radians.iter().take(4).enumerate() {
        ubo.azimuths[i] = azimuth;
    }

    // Pack shadow colors (4 colors * 4 components = 16 floats)
    for (i, color) in illumination.shadow_colors.iter().take(4).enumerate() {
        ubo.shadows[i * 4] = color.r;
        ubo.shadows[i * 4 + 1] = color.g;
        ubo.shadows[i * 4 + 2] = color.b;
        ubo.shadows[i * 4 + 3] = color.a;
    }

    // Pack highlight colors (4 colors * 4 components = 16 floats)
    for (i, color) in illumination.highlight_colors.iter().take(4).enumerate() {
        ubo.highlights[i * 4] = color.r;
        ubo.highlights[i * 4 + 1] = color.g;
        ubo.highlights[i * 4 + 2] = color.b;
        ubo.highlights[i * 4 + 3] = color.a;
    }

    ubo
}

/// Convert `HillshadeMethodType` enum to int for shader
fn method_to_int(method: HillshadeMethodType) -> i32 {
    method as i32
}

/// Calculate latitude range for tile
pub(crate) fn get_lat_range(id: &UnwrappedTileID) -> [f32; 2] {
    let latlng0 = LatLng::from_unwrapped_tile(id.canonical.z, id.canonical.x, id.canonical.y, id.wrap);
    let below = UnwrappedTileID::new(id.canonical.z, i64::from(id.canonical.x), i64::from(id.canonical.y) + 1);
    let latlng1 = LatLng::from_unwrapped_tile(below.canonical.z, below.canonical.x, below.canonical.y, below.wrap);
    [latlng0.latitude() as f32, latlng1.latitude() as f32]
}

pub(crate) struct HillshadeLayerTweaker {
    base: LayerTweakerBase,
    evaluated_props_uniform_buffer: Option<UniformBufferPtr>,
    drawable_uniform_buffer: Option<UniformBufferPtr>,
    tile_props_uniform_buffer: Option<UniformBufferPtr>,
}

impl HillshadeLayerTweaker {
    pub(crate) fn new(id: String, properties: Arc<dyn LayerProperties>) -> Self {
        HillshadeLayerTweaker {
            base: LayerTweakerBase::new(id, properties),
            evaluated_props_uniform_buffer: None,
            drawable_uniform_buffer: None,
            tile_props_uniform_buffer: None,
        }
    }
}

impl LayerTweaker for HillshadeLayerTweaker {
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

        let properties = Arc::clone(&self.base.evaluated_properties);
        let evaluated = get_evaluated::<HillshadeLayerProperties>(&properties);

        // Get illumination properties
        let mut illumination = get_illumination_properties(evaluated);

        // Adjust azimuths if anchor is viewport
        if *evaluated.get::<HillshadeIlluminationAnchor>() == HillshadeIlluminationAnchorType::Viewport {
            let bearing = parameters.state.get_bearing() as f32;
            for azimuth in &mut illumination.direction_radians {
                *azimuth -= bearing;
            }
        }

        let context = parameters.context;
        if self.evaluated_props_uniform_buffer.is_none() || self.base.properties_updated {
            let evaluated_props_ubo = pack_evaluated_props(&illumination, evaluated.get::<HillshadeAccentColor>());
            context
                .emplace_or_update_uniform_buffer_pod(&mut self.evaluated_props_uniform_buffer, &evaluated_props_ubo);
            self.base.properties_updated = false;
        }
        layer_group
            .mutable_uniform_buffers()
            .set(ID_HILLSHADE_EVALUATED_PROPS_UBO, self.evaluated_props_uniform_buffer.clone());

        let mut i: u32 = 0;
        let count = layer_group.get_drawable_count();
        let mut drawable_ubo_vector = vec![HillshadeDrawableUBO::default(); count];
        let mut projection_ubo_vector = vec![ProjectionUBO::default(); count];
        let mut tile_props_ubo_vector = vec![HillshadeTilePropsUBO::default(); count];

        let exaggeration = *evaluated.get::<HillshadeExaggeration>();
        let method = method_to_int(*evaluated.get::<HillshadeMethod>());
        let num_lights = illumination.num_sources() as i32;

        let self_ptr: *const Self = self;
        visit_layer_group_drawables(layer_group, |drawable| {
            let Some(tile_id) = drawable.get_tile_id() else {
                return;
            };
            if !check_tweak_drawable_ptr(drawable, self_ptr) {
                return;
            }

            let tile_id = tile_id.to_unwrapped();

            let projection = get_projection_data(
                &tile_id,
                parameters,
                [0.0, 0.0],
                TranslateAnchorType::Viewport,
                false,
                false,
                drawable,
                true,
            );
            let matrix = projection.fallback_matrix;
            projection_ubo_vector[i as usize] = to_projection_ubo(&projection);

            drawable_ubo_vector[i as usize] = HillshadeDrawableUBO { matrix: cast_matrix(&matrix) };

            tile_props_ubo_vector[i as usize] = HillshadeTilePropsUBO {
                latrange: get_lat_range(&tile_id),
                exaggeration,
                method,
                num_lights,
                pad0: 0.0,
                pad1: 0.0,
                pad2: 0.0,
            };

            drawable.set_ubo_index(i);
            i += 1;
        });

        let bytes: &[u8] = bytemuck::cast_slice(&drawable_ubo_vector);
        match &self.drawable_uniform_buffer {
            Some(buffer) if buffer.get_size() >= bytes.len() => buffer.update(bytes),
            _ => {
                self.drawable_uniform_buffer = Some(context.create_uniform_buffer(bytes, false, true));
            }
        }

        let bytes: &[u8] = bytemuck::cast_slice(&tile_props_ubo_vector);
        match &self.tile_props_uniform_buffer {
            Some(buffer) if buffer.get_size() >= bytes.len() => buffer.update(bytes),
            _ => {
                self.tile_props_uniform_buffer = Some(context.create_uniform_buffer(bytes, false, true));
            }
        }

        let layer_uniforms = layer_group.mutable_uniform_buffers();
        layer_uniforms.set(ID_HILLSHADE_DRAWABLE_UBO, self.drawable_uniform_buffer.clone());
        layer_uniforms.set(ID_HILLSHADE_TILE_PROPS_UBO, self.tile_props_uniform_buffer.clone());
        self.base.upload_projection_ubos(layer_uniforms, &projection_ubo_vector, context);
    }
}
