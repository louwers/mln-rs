use std::f64::consts::PI;

use glam::{DMat3, DVec3};
use mln_gfx::color_mode::ColorMode;
use mln_gfx::cull_face_mode::CullFaceMode;
use mln_gfx::gpu::{Gpu, ShaderKey};
use mln_gfx::projection_variant::ProjectionVariant;
use mln_gfx::{AttributeDataType, DepthMaskType, DrawMode};
use mln_style::color::Color;
use mln_style::geo::WrapMode;
use mln_style::types::LightAnchorType;

use crate::camera::View;
use crate::draw::context::Context;
use crate::draw::identity::SimpleIdentity;
use crate::draw::indexes::DrawIndexes;
use crate::draw::objects::{DrawGeometry, DrawObjects, Textures, raw_vertices};
use crate::draw::segment::SegmentBase;
use crate::draw::vertex_attribute::Uniforms;
use crate::layer::layer_drawing::{DrawDepth, FrameDraw, SingleDraw};
use crate::render::light::EvaluatedLight;
use crate::render::sky::EvaluatedSky;
use crate::render::static_data::RenderStaticData;
use crate::render::tile_matrix::bind_no_projection;
use crate::shaders::programs;
use crate::shaders::ubo::{AtmospherePropsUBO, SkyPropsUBO};

/// The ids of the buffers that the draws bind.
const SKY_PROPS_ID: usize = programs::SKY.buffer_id("sky");
const ATMOSPHERE_PROPS_ID: usize = programs::ATMOSPHERE.buffer_id("atmosphere");

/// A draw of `shader`, which reads the view's corners as the vertex attribute
/// `vertex_attribute`, in the translucent pass at `depth`.
fn fullscreen_draw(shader: ShaderKey, vertex_attribute: &'static str, depth: DrawDepth) -> SingleDraw {
    let corners: [[i16; 2]; 4] = [[-1, -1], [1, -1], [-1, 1], [1, 1]];
    let vertices = bytemuck::cast_slice(&corners).to_vec();
    let geometry = DrawGeometry {
        vertices: raw_vertices(vertex_attribute, vertices, corners.len(), AttributeDataType::Short2),
        vertex_count: corners.len(),
        mode: DrawMode::Triangles(),
        indexes: DrawIndexes::new(RenderStaticData::quad_triangle_indices()),
        segments: vec![SegmentBase::with_lengths(0, 0, 4, 6)],
        instances: None,
    };
    let objects =
        DrawObjects::new(shader, ColorMode::alpha_blended(), CullFaceMode::disabled(), geometry, Textures::default());
    SingleDraw::new(objects, FrameDraw { depth, ..FrameDraw::new(None, SimpleIdentity::new()) })
}

/// The direction of the sun in the globe's frame, where the atmosphere traces
/// its rays (`get_inverse_globe_view_projection_matrix`). A map-anchored light
/// is fixed to the globe, so its direction is already in that frame. A
/// viewport-anchored light is fixed to the view, so it goes back through the
/// globe's rotation into the view, inverted.
fn sun_position(light: &EvaluatedLight, state: &View, blend: f32) -> [f32; 4] {
    let position = light.light_position.uniform().get_cartesian();
    let mut sun = -DVec3::new(f64::from(position[0]), f64::from(position[1]), f64::from(position[2]));

    if *light.light_anchor.uniform() == LightAnchorType::Viewport {
        let center = state.center(WrapMode::Unwrapped);
        let view_to_globe = DMat3::from_rotation_y(center.longitude() * PI / 180.0)
            * DMat3::from_rotation_x(-center.latitude() * PI / 180.0)
            * DMat3::from_rotation_z(state.angle())
            * DMat3::from_rotation_x(state.pitch())
            * DMat3::from_rotation_z(-state.roll())
            * DMat3::from_rotation_z(state.north_orientation_angle());
        sun = view_to_globe * sun;
    }

    sun.as_vec3().extend(blend).to_array()
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
}

impl SkyPass {
    #[expect(
        clippy::too_many_arguments,
        reason = "the context and camera, the viewport, and the evaluated sky and light that the pass draws"
    )]
    pub(crate) fn update(
        &mut self,
        gpu: &mut dyn Gpu,
        context: &mut Context,
        state: &View,
        viewport_size: mln_gfx::Size,
        pixel_ratio: f32,
        evaluated: Option<&EvaluatedSky>,
        light: &EvaluatedLight,
    ) {
        let Some(evaluated) = evaluated.filter(|_| !viewport_size.is_empty()) else {
            self.sky = None;
            self.atmosphere = None;
            return;
        };

        let projection_transition = state.projection_transition().clamp(0.0, 1.0);
        if projection_transition < 1.0 {
            let sky = self.sky.get_or_insert_with(|| {
                let shader = context.shader_key(&programs::SKY, Uniforms::default(), ProjectionVariant::Mercator);
                let depth = DrawDepth::Layer { sub_layer: 0, mask: DepthMaskType::ReadWrite };
                fullscreen_draw(shader, "pos", depth)
            });
            let horizon_distance = state.mercator_horizon() * f64::from(pixel_ratio);
            let roll = state.roll();
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
            sky.write_pod(gpu, SKY_PROPS_ID, &props, context);
            bind_no_projection(gpu, &mut sky.frame.uniform_buffers, context);
        } else {
            self.sky = None;
        }

        let atmosphere_blend = *evaluated.sky_atmosphere_blend.uniform() * projection_transition as f32;
        if !state.is_globe() || atmosphere_blend <= 0.0 {
            self.atmosphere = None;
            return;
        }

        let atmosphere = self.atmosphere.get_or_insert_with(|| {
            let shader = context.shader_key(&programs::ATMOSPHERE, Uniforms::default(), ProjectionVariant::Globe);
            fullscreen_draw(shader, "pos", DrawDepth::Volume { mask: DepthMaskType::ReadOnly })
        });

        let camera = state.globe_camera_position();
        let props = AtmospherePropsUBO {
            inv_view_projection: state.inverse_globe_view_projection_matrix().as_mat4().to_cols_array(),
            camera_position: camera.as_vec3().extend(0.0).to_array(),
            sun_position: sun_position(light, state, atmosphere_blend),
        };
        atmosphere.write_pod(gpu, ATMOSPHERE_PROPS_ID, &props, context);
        bind_no_projection(gpu, &mut atmosphere.frame.uniform_buffers, context);
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
    use mln_style::geo::LatLng;
    use mln_style::property::evaluated::Evaluated;
    use mln_style::types::Position;

    use super::*;
    use crate::camera::{NorthOrientation, TransformState};

    fn light(anchor: LightAnchorType, position: [f32; 3]) -> EvaluatedLight {
        EvaluatedLight {
            light_anchor: Evaluated::Constant(anchor),
            light_position: Evaluated::Constant(Position::new(position)),
            ..EvaluatedLight::default()
        }
    }

    fn camera(latitude: f64, longitude: f64, bearing: f64) -> TransformState {
        let mut state = TransformState::default();
        state.set_size(mln_style::size::Size::new(512, 512));
        state.set_lat_lng_zoom(&LatLng::new(latitude, longitude).unwrap(), 1.0);
        state.set_angle(bearing);
        state
    }

    fn assert_sun(expected: [f32; 3], sun: [f32; 4]) {
        for (expected, actual) in expected.iter().zip(sun) {
            assert!((expected - actual).abs() < 1e-5, "expected {expected:?}, got {sun:?}");
        }
    }

    // In the globe's frame +x points to 0°N 90°E, +y to the North Pole and
    // +z to 0°N 0°E.

    // As the render test projection/globe/atmosphere/light-position-map
    // shows: wherever the
    // camera is, a map light at [1.5, 90, 90] stays over 0°N 90°E.
    #[test]
    fn map_light_stays_over_one_place() {
        let light = light(LightAnchorType::Map, [1.5, 90.0, 90.0]);
        for state in [camera(0.0, 0.0, 0.0), camera(0.0, 160.0, 0.0), camera(30.0, 160.0, 0.6)] {
            assert_sun([1.5, 0.0, 0.0], sun_position(&light, state.view(), 1.0));
        }
    }

    // As the render test projection/globe/atmosphere/light-position-viewport
    // shows: a
    // viewport light at [1.5, 90, 90] stays on the right of the view, which
    // is east of the center: over 0°N 90°E seen from 0°N 0°E, over 0°N 180°
    // seen from 0°N 90°E.
    #[test]
    fn viewport_light_stays_on_screen() {
        let light = light(LightAnchorType::Viewport, [1.5, 90.0, 90.0]);
        assert_sun([1.5, 0.0, 0.0], sun_position(&light, camera(0.0, 0.0, 0.0).view(), 1.0));
        assert_sun([0.0, 0.0, -1.5], sun_position(&light, camera(0.0, 90.0, 0.0).view(), 1.0));
    }

    // With north down, the right of the view is west of the center: the same
    // viewport light is over 0°N 90°W seen from 0°N 0°E.
    #[test]
    fn viewport_light_turns_with_the_north_orientation() {
        let light = light(LightAnchorType::Viewport, [1.5, 90.0, 90.0]);
        let mut state = camera(0.0, 0.0, 0.0);
        state.set_north_orientation(NorthOrientation::Downwards);
        assert_sun([-1.5, 0.0, 0.0], sun_position(&light, state.view(), 1.0));
    }
}
