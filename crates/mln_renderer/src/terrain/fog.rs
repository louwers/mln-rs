//! The fog over the terrain's surface, from the style's `sky`.

use mln_map_types::View;
use mln_style::color::Color;

use crate::render_sky::EvaluatedSky;
use crate::terrain::ubo::TerrainFog;

/// The fog over the surface in `view` under `sky`, or none without a sky.
///
/// The fog's depth runs, as GL JS's does, from the camera's distance to sea
/// level to the far plane, which the view fits to its horizon. A view
/// pitched less than 60° shows no horizon, and its far plane is just past
/// the top of the view, so the fog fades in from 60° to 70°. It fades out
/// as the view turns to the globe, whose atmosphere covers its limb.
pub(crate) fn terrain_fog(sky: Option<&EvaluatedSky>, view: &View) -> TerrainFog {
    let Some(sky) = sky else {
        return TerrainFog::NONE;
    };
    let pitch = view.pitch().to_degrees();
    let opacity = ((pitch - 60.0) / 10.0).clamp(0.0, 1.0) * (1.0 - view.projection_transition()).clamp(0.0, 1.0);
    if opacity <= 0.0 {
        return TerrainFog::NONE;
    }
    let opacity = opacity as f32;
    TerrainFog {
        fog_color: linear(sky.sky_fog_color.uniform(), opacity),
        horizon_color: linear(sky.sky_horizon_color.uniform(), opacity),
        depth_range: [view.camera_to_sea_level_distance() as f32, view.far_z() as f32],
        ground_blend: sky.sky_fog_ground_blend.uniform().clamp(0.0, 1.0),
        horizon_blend: sky.sky_horizon_fog_blend.uniform().clamp(0.0, 1.0),
    }
}

/// `color`, premultiplied, in linear components through a gamma of 2.2,
/// premultiplied by its alpha times `opacity`.
fn linear(color: &Color, opacity: f32) -> [f32; 4] {
    let alpha = color.a * opacity;
    if alpha <= 0.0 {
        return [0.0; 4];
    }
    let linear = |component: f32| (component / color.a).powf(2.2) * alpha;
    [linear(color.r), linear(color.g), linear(color.b), alpha]
}

#[cfg(test)]
mod tests {
    use mln_map_types::TransformState;
    use mln_style::property::evaluated::Evaluated;
    use mln_style::types::ProjectionDefinition;
    use mln_util::geo::LatLng;
    use mln_util::size::Size;

    use super::*;

    /// A sky of a translucent orange fog that starts halfway through the
    /// depth, toward a blue horizon from 0.8.
    fn sky() -> EvaluatedSky {
        EvaluatedSky {
            sky_fog_color: Evaluated::Constant(Color::new(0.5, 0.25, 0.0, 0.5)),
            sky_horizon_color: Evaluated::Constant(Color::new(0.0, 0.0, 1.0, 1.0)),
            sky_fog_ground_blend: Evaluated::Constant(0.5),
            sky_horizon_fog_blend: Evaluated::Constant(0.8),
            ..EvaluatedSky::default()
        }
    }

    /// A view of terrain pitched `pitch` degrees, its center 1 km up.
    fn pitched(pitch: f64) -> TransformState {
        let mut state = TransformState::default();
        state.set_size(Size::new(512, 384));
        state.set_lat_lng_zoom(&LatLng::new(46.5, 7.9).unwrap(), 12.0);
        state.set_center_altitude(1000.0);
        state.set_pitch(pitch.to_radians());
        state.set_terrain_min_elevation(Some(500.0));
        state
    }

    fn opacity(fog: &TerrainFog) -> f32 {
        fog.horizon_color[3]
    }

    #[test]
    fn the_fog_fades_in_from_60_to_70_degrees_of_pitch() {
        for (pitch, expected) in [(59.0, 0.0), (60.0, 0.0), (65.0, 0.5), (70.0, 1.0), (85.0, 1.0)] {
            let fog = terrain_fog(Some(&sky()), pitched(pitch).view());
            assert!((opacity(&fog) - expected).abs() < 1e-6, "{} at {pitch}°", opacity(&fog));
        }
        assert_eq!(TerrainFog::NONE, terrain_fog(Some(&sky()), pitched(30.0).view()));
    }

    #[test]
    fn there_is_no_fog_without_a_sky() {
        assert_eq!(TerrainFog::NONE, terrain_fog(None, pitched(85.0).view()));
    }

    #[test]
    fn the_fog_fades_out_as_the_view_turns_to_the_globe() {
        let mut state = pitched(85.0);
        state.set_projection_definition(&ProjectionDefinition::between_names("mercator", "vertical-perspective", 0.25));
        assert!((opacity(&terrain_fog(Some(&sky()), state.view())) - 0.75).abs() < 1e-6);
        state.set_projection_definition(&ProjectionDefinition::from_name("vertical-perspective"));
        assert_eq!(TerrainFog::NONE, terrain_fog(Some(&sky()), state.view()));
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
        assert!(fog.depth_range[0] < fog.depth_range[1], "{:?}", fog.depth_range);
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
        assert_eq!((0.0, 1.0), (fog.ground_blend, fog.horizon_blend));
    }
}
