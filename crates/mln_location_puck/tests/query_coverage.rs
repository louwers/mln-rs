//! A rendered-feature query finds a puck where it draws
//! ([`render_test::coverage`]): flat, pitched, lifted, on the globe away
//! from the center's latitude and near the horizon, and over terrain where
//! a hill hides it.

use mln_engine::RegistryBuilder;
use render_test::coverage::{COLOR, Camera, FLAT, GLOBE, HORIZON, PITCHED, Scene, TERRAIN, hill, style};

/// A scene of a [`COLOR`] puck at `position` seen by `camera`, with the
/// paint properties `extra` too, over [`hill`] where `terrain`.
fn puck(camera: Camera, projection: &str, terrain: bool, position: [f64; 2], extra: &str) -> Scene {
    let [lng, lat] = position;
    let layer = format!(
        r#"{{"id": "layer", "type": "org.maplibre.location-puck", "paint": {{
            "puck-position": [{lng}, {lat}], "puck-radius": 14, "puck-color": "{COLOR}",
            "puck-border-color": "{COLOR}"{extra}
        }}}}"#
    );
    let ground = terrain.then(hill);
    let empty = r#"{"type": "FeatureCollection", "features": []}"#;
    Scene { style: style(camera, projection, terrain, empty, &layer), layer: "layer".to_owned(), ground, shows: true }
}

#[test]
fn a_query_finds_a_puck_where_it_draws() {
    let mut builder = RegistryBuilder::new();
    mln_builtins::register_builtins(&mut builder).unwrap();
    mln_location_puck::register(&mut builder).unwrap();
    let arrow = &format!(
        r#", "puck-bearing-visible": true, "puck-bearing": 30, "puck-bearing-radius": 30,
        "puck-bearing-arrow-color": "{COLOR}""#
    );
    let lifted = r#", "puck-tilt-displacement": 0.5"#;
    let output = mln_headless::test_output_dir(env!("CARGO_TARGET_TMPDIR")).join("puck_query_coverage");
    render_test::coverage::assert_matches(
        &builder.build(),
        &output,
        vec![
            ("puck-flat", puck(FLAT, "mercator", false, [0.01, 0.0], "")),
            ("puck-arrow", puck(FLAT, "mercator", false, [0.01, 0.0], arrow)),
            ("puck-pitched", puck(PITCHED, "mercator", false, [0.01, 0.01], arrow)),
            ("puck-lifted", puck(PITCHED, "mercator", false, [0.01, 0.01], lifted)),
            ("puck-globe", puck(GLOBE, "globe", false, [5.0, 60.0], arrow)),
            ("puck-horizon", puck(HORIZON, "globe", false, [-6.0, 10.0], "")),
            ("puck-terrain-hidden", Scene { shows: false, ..puck(TERRAIN, "mercator", true, [0.0, 0.007], "") }),
            ("puck-terrain", puck(TERRAIN, "mercator", true, [0.006, 0.0], "")),
        ],
    );
}
