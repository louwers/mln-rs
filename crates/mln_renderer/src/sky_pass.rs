use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;

use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::index_vector::IndexVectorBase;
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::shaders::shader_defines::{
    ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, ID_ATMOSPHERE_PROPS_UBO, ID_SKY_POS_VERTEX_ATTRIBUTE, ID_SKY_PROPS_UBO,
};
use mln_gfx::shaders::sky_ubo::{AtmospherePropsUBO, SkyPropsUBO};
use mln_gfx::{AttributeDataType, DepthMaskType, DrawMode, ShaderProgramPtr, ShaderRegistry};
use mln_map_types::TransformState;
use mln_style_types::LightAnchorType;
use mln_util::Color;
use mln_util::geo::WrapMode;
use mln_util::identity::SimpleIdentity;
use mln_util::mat4::{Mat4, matrix};
use mln_util::size::Size;
use mln_util::vectors::Vec4;

use crate::draw::{DrawGeometry, DrawObjects, Textures, raw_vertices};
use crate::gfx::context::Context;
use crate::layer_drawing::{DrawDepth, FrameDraw, SingleDraw};
use crate::render_light::EvaluatedLight;
use crate::render_sky::EvaluatedSky;
use crate::render_static_data::RenderStaticData;
use crate::tile_matrix::bind_no_projection;

const SKY_SHADER_NAME: &str = "SkyShader";
const ATMOSPHERE_SHADER_NAME: &str = "AtmosphereShader";

/// A draw of `shader`, which reads the view's corners as the vertex attribute
/// `vertex_attribute`, in the translucent pass at `depth`.
fn fullscreen_draw(shader: &ShaderProgramPtr, vertex_attribute: usize, depth: DrawDepth) -> SingleDraw {
    let corners: [[i16; 2]; 4] = [[-1, -1], [1, -1], [-1, 1], [1, 1]];
    let vertices = bytemuck::cast_slice(&corners).to_vec();
    let indexes = IndexVectorBase::new(RenderStaticData::quad_triangle_indices());
    let geometry = DrawGeometry {
        vertices: raw_vertices(vertex_attribute, vertices, corners.len(), AttributeDataType::Short2),
        vertex_count: corners.len(),
        mode: DrawMode::Triangles(),
        indexes: Rc::new(RefCell::new(indexes)),
        segments: vec![SegmentBase::with_lengths(0, 0, 4, 6)],
        instances: None,
    };
    let objects = DrawObjects::new(
        Rc::clone(shader),
        ColorMode::alpha_blended(),
        CullFaceMode::disabled(),
        geometry,
        Textures::default(),
    );
    SingleDraw::new(objects, FrameDraw { depth, ..FrameDraw::new(None, SimpleIdentity::new()) })
}

/// The direction of the sun in the globe's frame, where the atmosphere traces
/// its rays (`get_inverse_globe_view_projection_matrix`). A map-anchored light
/// is fixed to the globe, so its direction is already in that frame. A
/// viewport-anchored light is fixed to the view, so it goes back through the
/// globe's rotation into the view, inverted.
fn sun_position(light: &EvaluatedLight, state: &TransformState, blend: f32) -> [f32; 4] {
    let position = light.light_position.uniform().get_cartesian();
    let mut sun: Vec4 = [-f64::from(position[0]), -f64::from(position[1]), -f64::from(position[2]), 0.0];

    if *light.light_anchor.uniform() == LightAnchorType::Viewport {
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
        let a = m;
        matrix::rotate_z(&mut m, &a, state.get_north_orientation_angle());
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
pub(crate) struct SkyPass {
    sky: Option<SingleDraw>,
    atmosphere: Option<SingleDraw>,
    sky_shader: Option<ShaderProgramPtr>,
    atmosphere_shader: Option<ShaderProgramPtr>,
}

impl SkyPass {
    #[expect(
        clippy::too_many_arguments,
        reason = "the shaders, context and camera, the viewport, and the evaluated sky and light that the pass draws"
    )]
    pub(crate) fn update(
        &mut self,
        shaders: &ShaderRegistry,
        context: &Context,
        state: &TransformState,
        viewport_size: Size,
        pixel_ratio: f32,
        evaluated: Option<&EvaluatedSky>,
        light: &EvaluatedLight,
    ) {
        let Some(evaluated) = evaluated.filter(|_| !viewport_size.is_empty()) else {
            self.sky = None;
            self.atmosphere = None;
            return;
        };

        let projection_transition = state.get_projection_transition().clamp(0.0, 1.0);
        if projection_transition < 1.0 {
            if self.sky_shader.is_none() {
                self.sky_shader = shaders.shader(SKY_SHADER_NAME, ProjectionVariant::Mercator);
            }
            if self.sky.is_none() {
                let depth = DrawDepth::Layer { sub_layer: 0, mask: DepthMaskType::ReadWrite };
                self.sky =
                    self.sky_shader.as_ref().map(|shader| fullscreen_draw(shader, ID_SKY_POS_VERTEX_ATTRIBUTE, depth));
            }

            if let Some(sky) = &mut self.sky {
                let horizon_distance = state.get_mercator_horizon() * f64::from(pixel_ratio);
                let roll = state.get_roll();
                let normal = [(-roll.sin()) as f32, roll.cos() as f32];
                let horizon = [
                    (f64::from(viewport_size.width) * 0.5 + f64::from(normal[0]) * horizon_distance) as f32,
                    (f64::from(viewport_size.height) * 0.5 + f64::from(normal[1]) * horizon_distance) as f32,
                ];

                let props = SkyPropsUBO {
                    sky_color: color_components(evaluated.sky_color.uniform()),
                    horizon_color: color_components(evaluated.sky_horizon_color.uniform()),
                    horizon,
                    horizon_normal: normal,
                    viewport_size: [viewport_size.width as f32, viewport_size.height as f32],
                    sky_horizon_blend: *evaluated.sky_horizon_blend.uniform() * viewport_size.height as f32 * 0.5,
                    sky_blend: projection_transition as f32,
                };
                let buffers = &mut sky.frame.uniform_buffers;
                buffers.create_or_update_pod(ID_SKY_PROPS_UBO, &props, context);
                bind_no_projection(buffers, context);
            }
        } else {
            self.sky = None;
        }

        let atmosphere_blend = *evaluated.sky_atmosphere_blend.uniform() * projection_transition as f32;
        if !state.is_globe_rendering() || atmosphere_blend <= 0.0 {
            self.atmosphere = None;
            return;
        }

        if self.atmosphere_shader.is_none() {
            self.atmosphere_shader = shaders.shader(ATMOSPHERE_SHADER_NAME, ProjectionVariant::Globe);
        }
        if self.atmosphere.is_none() {
            let depth = DrawDepth::Volume { mask: DepthMaskType::ReadOnly };
            self.atmosphere = self
                .atmosphere_shader
                .as_ref()
                .map(|shader| fullscreen_draw(shader, ID_ATMOSPHERE_POS_VERTEX_ATTRIBUTE, depth));
        }
        let Some(atmosphere) = &mut self.atmosphere else {
            return;
        };

        let camera = state.get_globe_camera_position();
        let props = AtmospherePropsUBO {
            inv_view_projection: state.get_inverse_globe_view_projection_matrix().map(|v| v as f32),
            camera_position: [camera[0] as f32, camera[1] as f32, camera[2] as f32, 0.0],
            sun_position: sun_position(light, state, atmosphere_blend),
        };
        let buffers = &mut atmosphere.frame.uniform_buffers;
        buffers.create_or_update_pod(ID_ATMOSPHERE_PROPS_UBO, &props, context);
        bind_no_projection(buffers, context);
    }

    /// The planar sky, which draws before the layers, if the frame draws it.
    #[must_use]
    pub(crate) fn sky(&mut self) -> Option<&mut SingleDraw> {
        self.sky.as_mut()
    }

    /// The globe's atmosphere, which draws after the layers, if the frame
    /// draws it.
    #[must_use]
    pub(crate) fn atmosphere(&mut self) -> Option<&mut SingleDraw> {
        self.atmosphere.as_mut()
    }

    #[must_use]
    pub(crate) fn has_atmosphere(&self) -> bool {
        self.atmosphere.is_some()
    }
}

#[cfg(test)]
mod tests {
    use mln_map_types::NorthOrientation;
    use mln_style::evaluated::Evaluated;
    use mln_style_types::Position;
    use mln_util::geo::LatLng;

    use super::*;

    fn light(anchor: LightAnchorType, position: [f32; 3]) -> EvaluatedLight {
        EvaluatedLight {
            light_anchor: Evaluated::Constant(anchor),
            light_position: Evaluated::Constant(Position::new(position)),
            ..EvaluatedLight::default()
        }
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

    // With north down, the right of the view is west of the center: the same
    // viewport light is over 0°N 90°W seen from 0°N 0°E.
    #[test]
    fn viewport_light_turns_with_the_north_orientation() {
        let light = light(LightAnchorType::Viewport, [1.5, 90.0, 90.0]);
        let mut state = camera(0.0, 0.0, 0.0);
        state.set_north_orientation(NorthOrientation::Downwards);
        assert_sun([-1.5, 0.0, 0.0], sun_position(&light, &state, 1.0));
    }
}
