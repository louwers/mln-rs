//! The terrain's fog: GL JS's, from the frame's sky and view.

use mln_gfx::shaders::terrain_ubo::TerrainFogUBO;
use mln_map_types::View;
use mln_util::Color;

use crate::render_sky::EvaluatedSky;

/// The fog over the surface of a frame of `view` under `sky`: GL JS's fog
/// colors and blends, its opacity by the pitch (`calculateFogBlendOpacity`),
/// and the depth range of its fog matrix, from the camera's distance to sea
/// level to the far plane. The blends are clamped to 0 and 1, and
/// the colors are linear, through a gamma of 2.2, and premultiplied. Without a
/// sky, in an axonometric view, whose clip w is no distance, with a ground
/// blend of 1, or with a range that ends where it starts, there is no fog.
pub(crate) fn terrain_fog(sky: Option<&EvaluatedSky>, view: &View) -> TerrainFogUBO {
    let Some(sky) = sky else {
        return TerrainFogUBO::default();
    };
    let depth_range = [view.camera_to_sea_level_distance() as f32, view.far_z() as f32];
    let ground_blend = sky.sky_fog_ground_blend.uniform().clamp(0.0, 1.0);
    let pitch = mln_math::rad2deg(view.pitch());
    let opacity = if view.projection_mode().axonometric == Some(true)
        || pitch < 60.0
        || ground_blend >= 1.0
        || depth_range[1] <= depth_range[0]
    {
        0.0
    } else if pitch < 70.0 {
        (pitch - 60.0) / 10.0
    } else {
        1.0
    };
    TerrainFogUBO {
        fog_color: linear(sky.sky_fog_color.uniform()),
        horizon_color: linear(sky.sky_horizon_color.uniform()),
        depth_range,
        ground_blend,
        horizon_blend: sky.sky_horizon_fog_blend.uniform().clamp(0.0, 1.0),
        opacity: opacity as f32,
        pad: [0.0; 3],
    }
}

/// The linear components of `color`, premultiplied as it is.
fn linear(color: &Color) -> [f32; 4] {
    let [r, g, b, a] = [color.r, color.g, color.b, color.a];
    if a <= 0.0 {
        return [0.0; 4];
    }
    let linear = |c: f32| (c / a).powf(2.2) * a;
    [linear(r), linear(g), linear(b), a]
}

#[cfg(test)]
mod tests {
    use mln_map_types::TransformState;
    use mln_style::evaluated::Evaluated;
    use mln_util::geo::{EdgeInsets, LatLng};
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
        let linear = |c: f32| c.powf(2.2) * 0.5;
        assert_eq!([linear(1.0), linear(0.5), 0.0, 0.5], fog.fog_color, "linear and premultiplied");
        assert_eq!([0.0, 0.0, 1.0, 1.0], fog.horizon_color);
        assert_eq!((0.5, 0.8), (fog.ground_blend, fog.horizon_blend));
        assert_eq!([view.camera_to_sea_level_distance() as f32, view.far_z() as f32], fog.depth_range);
        assert!(fog.depth_range[0] > 0.0 && fog.depth_range[0] < fog.depth_range[1], "{:?}", fog.depth_range);
    }

    #[test]
    fn a_transparent_fog_color_has_no_color() {
        let sky = EvaluatedSky { sky_fog_color: Evaluated::Constant(Color::new(0.0, 0.0, 0.0, 0.0)), ..sky() };
        assert_eq!([0.0; 4], terrain_fog(Some(&sky), pitched(75.0).view()).fog_color);
    }

    #[test]
    fn the_blends_are_clamped_to_0_and_1() {
        let sky = EvaluatedSky {
            sky_fog_ground_blend: Evaluated::Constant(-0.5),
            sky_horizon_fog_blend: Evaluated::Constant(1.5),
            ..sky()
        };
        let fog = terrain_fog(Some(&sky), pitched(75.0).view());
        assert_eq!((0.0, 1.0, 1.0), (fog.ground_blend, fog.horizon_blend, fog.opacity));
    }

    /// A ground blend of 1 starts the fog where the depth ends, and a large
    /// inset can pull the far plane in before the fog's near distance.
    #[test]
    fn there_is_no_fog_from_a_ground_blend_of_1_or_a_depth_range_that_ends_where_it_starts() {
        let sky_without_ground = EvaluatedSky { sky_fog_ground_blend: Evaluated::Constant(1.0), ..sky() };
        assert_eq!(0.0, terrain_fog(Some(&sky_without_ground), pitched(75.0).view()).opacity);

        let mut state = TransformState::default();
        state.set_size(Size::new(512, 512));
        state.set_pitch(mln_math::deg2rad(75.0));
        state.set_padding(&EdgeInsets { bottom: 400.0, ..EdgeInsets::default() });
        state.set_terrain_min_elevation(Some(0.0));
        let view = state.view();
        assert!(
            view.far_z() <= view.camera_to_sea_level_distance(),
            "{} to {}",
            view.camera_to_sea_level_distance(),
            view.far_z()
        );
        assert_eq!(0.0, terrain_fog(Some(&sky()), view).opacity);
    }
}
