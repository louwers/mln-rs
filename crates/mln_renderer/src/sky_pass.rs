// Ports src/mln/renderer/sky_pass.hpp and src/mln/renderer/sky_pass.cpp.

use std::f64::consts::PI;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::renderer::layer_group::LayerGroupBasePtr;
use mln_gfx::shaders::shader_defines::{
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_ATMOSPHERE_PROPS_UBO, ID_SKY_POS_VERTEX_ATTRIBUTE, ID_SKY_PROPS_UBO,
};
use mln_gfx::shaders::sky_ubo::{AtmospherePropsUBO, SkyPropsUBO};
use mln_gfx::{Context, DepthMaskType, ShaderProgramBasePtr, ShaderRegistry};
use mln_map_types::TransformState;
use mln_style::light::{LightAnchor, LightPosition};
use mln_style::sky::{SkyAtmosphereBlend, SkyColor, SkyHorizonBlend, SkyHorizonColor};
use mln_style_types::LightAnchorType;
use mln_util::Color;
use mln_util::geo::WrapMode;
use mln_util::mat4::{Mat4, matrix};
use mln_util::size::Size;
use mln_util::vectors::Vec4;

use crate::render_light::EvaluatedLight;
use crate::render_pass::RenderPass;
use crate::render_sky::EvaluatedSky;

const SKY_SHADER_NAME: &str = "SkyShader";
const ATMOSPHERE_SHADER_NAME: &str = "AtmosphereShader";

fn make_fullscreen_layer_group(
    context: &dyn Context,
    shader: &Option<ShaderProgramBasePtr>,
    name: &str,
    vertex_attribute: usize,
    is_3d: bool,
    depth_mask: DepthMaskType,
) -> Option<LayerGroupBasePtr> {
    shader.as_ref()?;

    let layer_group = context.create_layer_group(0, 1, name.to_owned())?;

    let mut builder = context.create_drawable_builder(name.to_owned());
    builder.set_shader(shader.clone());
    builder.set_render_pass(RenderPass::Translucent);
    builder.set_is_3d(is_3d);
    builder.set_depth_type(depth_mask);
    builder.set_enable_depth(true);
    builder.set_enable_stencil(false);
    builder.set_color_mode(&ColorMode::alpha_blended());
    builder.set_cull_face_mode(&CullFaceMode::disabled());
    builder.set_vertex_attr_id(vertex_attribute);
    builder.add_quad(-1, -1, 1, 1);
    builder.flush(context);

    {
        let mut group = layer_group.borrow_mut();
        let group = group.as_layer_group_mut().expect("a sky layer group is a LayerGroup");
        for drawable in builder.clear_drawables() {
            group.add_drawable(drawable);
        }
    }
    Some(layer_group)
}

/// The direction of the sun in the globe's frame, where the atmosphere traces
/// its rays (`get_inverse_globe_view_projection_matrix`). A map-anchored light
/// is fixed to the globe, so its direction is already in that frame. A
/// viewport-anchored light is fixed to the view, so it goes back through the
/// globe's rotation into the view (`globe_view_projection_matrix`), inverted.
///
/// Deviation: C++ turns the map-anchored light through that rotation and
/// leaves the viewport-anchored one as is, which is GL JS's `getSunPos`, but
/// GL JS traces the atmosphere in view space. So in C++ a viewport light stays
/// over one place on the globe and a map light moves across it with the
/// camera. GL JS's projection/globe/atmosphere/light-position-map and
/// ligth-position-viewport render tests show what the port draws;
/// render-tests/sky/atmosphere-globe, which expects the C++ lighting, is on
/// tools/render-test/ignores/rust-port.json.
fn sun_position(light: &EvaluatedLight, state: &TransformState, blend: f32) -> [f32; 4] {
    let position = light.get::<LightPosition>().get_cartesian();
    let mut sun: Vec4 = [-f64::from(position[0]), -f64::from(position[1]), -f64::from(position[2]), 0.0];

    if *light.get::<LightAnchor>() == LightAnchorType::Viewport {
        let center = state.get_lat_lng(WrapMode::Unwrapped);
        let mut m: Mat4 = [0.0; 16];
        matrix::identity(&mut m);
        let a = m;
        matrix::rotate_y(&mut m, &a, center.longitude() * PI / 180.0);
        let a = m;
        matrix::rotate_x(&mut m, &a, -center.latitude() * PI / 180.0);
        let a = m;
        matrix::rotate_z(&mut m, &a, state.get_bearing());
        let a = m;
        matrix::rotate_x(&mut m, &a, state.get_pitch());
        let a = m;
        matrix::rotate_z(&mut m, &a, -state.get_roll());
        let a = sun;
        matrix::transform_mat4(&mut sun, &a, &m);
    }

    [sun[0] as f32, sun[1] as f32, sun[2] as f32, blend]
}

fn color_components(color: &Color) -> [f32; 4] {
    [color.r, color.g, color.b, color.a]
}

/// Owns the source-independent fullscreen passes used by the root `sky` style property.
///
/// The planar sky is rendered before style layers. The physical atmosphere is rendered
/// after them, against the globe depth prepass, so the planet correctly occludes it.
#[derive(Default)]
pub struct SkyPass {
    sky_layer_group: Option<LayerGroupBasePtr>,
    atmosphere_layer_group: Option<LayerGroupBasePtr>,
    sky_shader: Option<ShaderProgramBasePtr>,
    atmosphere_shader: Option<ShaderProgramBasePtr>,
}

impl SkyPass {
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &dyn Context,
        state: &TransformState,
        viewport_size: &Size,
        pixel_ratio: f32,
        evaluated: Option<&EvaluatedSky>,
        light: &EvaluatedLight,
    ) {
        let Some(evaluated) = evaluated.filter(|_| !viewport_size.is_empty()) else {
            if let Some(sky_layer_group) = &self.sky_layer_group {
                sky_layer_group.borrow_mut().clear_drawables();
            }
            if let Some(atmosphere_layer_group) = &self.atmosphere_layer_group {
                atmosphere_layer_group.borrow_mut().clear_drawables();
            }
            return;
        };

        // `std::clamp(state.getProjectionTransition(), 0.0, 1.0)`.
        let transition = state.get_projection_transition();
        let projection_transition = if transition < 0.0 {
            0.0
        } else if 1.0 < transition {
            1.0
        } else {
            transition
        };
        if projection_transition < 1.0 {
            if self.sky_shader.is_none() {
                self.sky_shader = context.get_generic_shader(shaders, SKY_SHADER_NAME, ProjectionVariant::Mercator);
            }
            if self.sky_layer_group.as_ref().is_none_or(|group| group.borrow().empty()) {
                self.sky_layer_group = make_fullscreen_layer_group(
                    context,
                    &self.sky_shader,
                    "sky",
                    ID_SKY_POS_VERTEX_ATTRIBUTE,
                    false,
                    DepthMaskType::ReadWrite,
                );
            }

            if let Some(sky_layer_group) = &self.sky_layer_group {
                let max_horizon_angle = 89.25 * PI / 180.0;
                let pitch = state.get_pitch();
                // `std::min(a, b)`.
                let a = (PI / 2.0 - pitch).tan() * 0.85;
                let b = (max_horizon_angle - pitch).tan();
                let horizon_distance =
                    f64::from(state.get_camera_to_center_distance() * pixel_ratio) * if b < a { b } else { a };
                let roll = state.get_roll();
                let normal = [(-roll.sin()) as f32, roll.cos() as f32];
                let horizon = [
                    (f64::from(viewport_size.width) * 0.5 + f64::from(normal[0]) * horizon_distance) as f32,
                    (f64::from(viewport_size.height) * 0.5 + f64::from(normal[1]) * horizon_distance) as f32,
                ];

                let props = SkyPropsUBO {
                    sky_color: color_components(evaluated.get::<SkyColor>()),
                    horizon_color: color_components(evaluated.get::<SkyHorizonColor>()),
                    horizon,
                    horizon_normal: normal,
                    viewport_size: [viewport_size.width as f32, viewport_size.height as f32],
                    sky_horizon_blend: *evaluated.get::<SkyHorizonBlend>() * viewport_size.height as f32 * 0.5,
                    sky_blend: projection_transition as f32,
                };
                sky_layer_group.borrow_mut().mutable_uniform_buffers().create_or_update_pod(
                    ID_SKY_PROPS_UBO,
                    &props,
                    context,
                );
            }
        } else if let Some(sky_layer_group) = &self.sky_layer_group {
            sky_layer_group.borrow_mut().clear_drawables();
        }

        let atmosphere_blend = *evaluated.get::<SkyAtmosphereBlend>() * projection_transition as f32;
        if !state.is_globe_rendering() || atmosphere_blend <= 0.0 {
            if let Some(atmosphere_layer_group) = &self.atmosphere_layer_group {
                atmosphere_layer_group.borrow_mut().clear_drawables();
            }
            return;
        }

        if self.atmosphere_shader.is_none() {
            self.atmosphere_shader =
                context.get_generic_shader(shaders, ATMOSPHERE_SHADER_NAME, ProjectionVariant::Globe);
        }
        if self.atmosphere_layer_group.as_ref().is_none_or(|group| group.borrow().empty()) {
            self.atmosphere_layer_group = make_fullscreen_layer_group(
                context,
                &self.atmosphere_shader,
                "atmosphere",
                ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE,
                true,
                DepthMaskType::ReadOnly,
            );
        }
        let Some(atmosphere_layer_group) = &self.atmosphere_layer_group else {
            return;
        };

        let camera = state.get_globe_camera_position();
        let props = AtmospherePropsUBO {
            inv_view_projection: state.get_inverse_globe_view_projection_matrix().map(|v| v as f32),
            camera_position: [camera[0] as f32, camera[1] as f32, camera[2] as f32, 0.0],
            sun_position: sun_position(light, state, atmosphere_blend),
        };
        atmosphere_layer_group.borrow_mut().mutable_uniform_buffers().create_or_update_pod(
            ID_ATMOSPHERE_PROPS_UBO,
            &props,
            context,
        );
    }

    pub fn get_sky_layer_group(&self) -> Option<&LayerGroupBasePtr> {
        self.sky_layer_group.as_ref()
    }

    pub fn get_atmosphere_layer_group(&self) -> Option<&LayerGroupBasePtr> {
        self.atmosphere_layer_group.as_ref()
    }

    pub fn has_atmosphere(&self) -> bool {
        self.atmosphere_layer_group.as_ref().is_some_and(|group| !group.borrow().empty())
    }
}

#[cfg(test)]
mod tests {
    use mln_style_types::Position;
    use mln_util::geo::LatLng;

    use super::*;

    fn light(anchor: LightAnchorType, position: [f32; 3]) -> EvaluatedLight {
        let mut light = EvaluatedLight::default();
        *light.get_mut::<LightAnchor>() = anchor;
        *light.get_mut::<LightPosition>() = Position::new(position);
        light
    }

    fn camera(latitude: f64, longitude: f64, bearing: f64) -> TransformState {
        let mut state = TransformState::default();
        state.set_size(Size::new(512, 512));
        state.set_lat_lng_zoom(&LatLng::new(latitude, longitude).unwrap(), 1.0);
        state.set_bearing(bearing);
        state
    }

    fn assert_sun(expected: [f32; 3], sun: [f32; 4]) {
        for (expected, actual) in expected.iter().zip(sun) {
            assert!((expected - actual).abs() < 1e-5, "expected {expected:?}, got {sun:?}");
        }
    }

    // In the globe's frame +x points to 0°N 90°E, +y to the North Pole and
    // +z to 0°N 0°E.

    // As GL JS's projection/globe/atmosphere/light-position-map: wherever the
    // camera is, a map light at [1.5, 90, 90] stays over 0°N 90°E.
    #[test]
    fn map_light_stays_over_one_place() {
        let light = light(LightAnchorType::Map, [1.5, 90.0, 90.0]);
        for state in [camera(0.0, 0.0, 0.0), camera(0.0, 160.0, 0.0), camera(30.0, 160.0, 0.6)] {
            assert_sun([1.5, 0.0, 0.0], sun_position(&light, &state, 1.0));
        }
    }

    // As GL JS's projection/globe/atmosphere/ligth-position-viewport: a
    // viewport light at [1.5, 90, 90] stays on the right of the view, which
    // is east of the center: over 0°N 90°E seen from 0°N 0°E, over 0°N 180°
    // seen from 0°N 90°E.
    #[test]
    fn viewport_light_stays_on_screen() {
        let light = light(LightAnchorType::Viewport, [1.5, 90.0, 90.0]);
        assert_sun([1.5, 0.0, 0.0], sun_position(&light, &camera(0.0, 0.0, 0.0), 1.0));
        assert_sun([0.0, 0.0, -1.5], sun_position(&light, &camera(0.0, 90.0, 0.0), 1.0));
    }
}
