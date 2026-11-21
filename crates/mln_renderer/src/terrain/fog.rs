//! The terrain's fog: GL JS's, from the frame's sky and view.

use mln_gfx::shaders::terrain_ubo::TerrainFogUBO;
use mln_map_types::View;
use mln_util::Color;

use crate::render_sky::EvaluatedSky;

/// The fog over the surface of a frame of `view` under `sky`, with `ground`
/// 0, which the draws of the first stack set: GL JS's fog colors and blends,
/// its opacity by the pitch (`calculateFogBlendOpacity`), and the depth range
/// of its fog matrix, from the camera's distance to sea level to the far
/// plane. Without a sky, and in an axonometric view, whose clip w is no
/// distance, there is no fog.
pub(crate) fn terrain_fog(sky: Option<&EvaluatedSky>, view: &View) -> TerrainFogUBO {
    let Some(sky) = sky else {
        return TerrainFogUBO::default();
    };
    let pitch = mln_math::rad2deg(view.pitch());
    let opacity = if view.projection_mode().axonometric == Some(true) || pitch < 60.0 {
        0.0
    } else if pitch < 70.0 {
        (pitch - 60.0) / 10.0
    } else {
        1.0
    };
    let components = |color: &Color| [color.r, color.g, color.b, color.a];
    TerrainFogUBO {
        fog_color: components(sky.sky_fog_color.uniform()),
        horizon_color: components(sky.sky_horizon_color.uniform()),
        depth_range: [view.camera_to_sea_level_distance() as f32, view.far_z() as f32],
        ground_blend: *sky.sky_fog_ground_blend.uniform(),
        horizon_blend: *sky.sky_horizon_fog_blend.uniform(),
        opacity: opacity as f32,
        ground: 0.0,
        pad: [0.0; 2],
    }
}

#[cfg(test)]
mod tests {
    use mln_map_types::TransformState;
    use mln_style::evaluated::Evaluated;
    use mln_util::geo::LatLng;
    use mln_util::size::Size;

    use super::*;

    /// A sky of a translucent orange fog that blends from the middle of the
    /// view's depth, toward a blue horizon from 0.8.
    fn sky() -> EvaluatedSky {
        EvaluatedSky {
            sky_fog_color: Evaluated::Constant(Color::new(0.5, 0.25, 0.0, 0.5)),
            sky_horizon_color: Evaluated::Constant(Color::new(0.0, 0.0, 1.0, 1.0)),
            sky_fog_ground_blend: Evaluated::Constant(0.5),
            sky_horizon_fog_blend: Evaluated::Constant(0.8),
            ..EvaluatedSky::default()
        }
    }

    /// A view of terrain pitched `pitch` degrees, with the center 1 km up.
    fn pitched(pitch: f64) -> TransformState {
        let mut state = TransformState::default();
        state.set_size(Size::new(512, 384));
        state.set_lat_lng_zoom(&LatLng::new(46.5, 7.9).unwrap(), 12.0);
        state.set_center_altitude(1000.0);
        state.set_pitch(mln_math::deg2rad(pitch));
        state.set_terrain_min_elevation(Some(500.0));
        state
    }

    #[test]
    fn the_fog_fades_in_from_60_to_70_degrees_of_pitch() {
        for (pitch, opacity) in [(0.0, 0.0), (59.0, 0.0), (60.0, 0.0), (65.0, 0.5), (70.0, 1.0), (85.0, 1.0)] {
            let fog = terrain_fog(Some(&sky()), pitched(pitch).view());
            assert!((fog.opacity - opacity).abs() < 1e-6, "{} at {pitch}°", fog.opacity);
        }
    }

    #[test]
    fn there_is_no_fog_without_a_sky_or_in_an_axonometric_view() {
        assert_eq!(TerrainFogUBO::default(), terrain_fog(None, pitched(85.0).view()));
        let mut state = pitched(85.0);
        state.set_axonometric(true);
        assert_eq!(0.0, terrain_fog(Some(&sky()), state.view()).opacity);
    }

    #[test]
    fn the_fog_takes_the_sky_s_colors_and_blends_and_the_view_s_depths() {
        let state = pitched(75.0);
        let view = state.view();
        let fog = terrain_fog(Some(&sky()), view);
        assert_eq!([0.5, 0.25, 0.0, 0.5], fog.fog_color, "premultiplied");
        assert_eq!([0.0, 0.0, 1.0, 1.0], fog.horizon_color);
        assert_eq!((0.5, 0.8), (fog.ground_blend, fog.horizon_blend));
        assert_eq!([view.camera_to_sea_level_distance() as f32, view.far_z() as f32], fog.depth_range);
        assert!(fog.depth_range[0] > 0.0 && fog.depth_range[0] < fog.depth_range[1], "{:?}", fog.depth_range);
        assert_eq!(0.0, fog.ground);
    }
}
