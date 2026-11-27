//! A rendered-feature query finds an n-gon where it draws
//! ([`render_test::coverage`]): flat, pitched, on the globe away from the
//! center's latitude and near the horizon, over terrain where a hill hides
//! one, aligned with the map or the viewport, and translated.

use mln_engine::RegistryBuilder;
use render_test::coverage::{COLOR, FLAT, GLOBE, HORIZON, PITCHED, TERRAIN, hill, layer, points, scene};

#[test]
fn a_query_finds_an_ngon_where_it_draws() {
    let mut builder = RegistryBuilder::new();
    mln_builtins::register_builtins(&mut builder).unwrap();
    mln_ngon::register(&mut builder).unwrap();
    let ngon = |extra: &str| {
        layer(
            "org.maplibre.ngon",
            &format!(r#""ngon-color": "{COLOR}", "ngon-radius": 16, "ngon-corners": 5, "ngon-blur": 0{extra}"#),
            "",
        )
    };
    let near = points(&[[-0.03, 0.0], [0.0, 0.02], [0.03, -0.01]]);
    let far = points(&[[-12.0, 58.0], [0.0, 62.0], [12.0, 57.0]]);
    let map = r#", "ngon-pitch-alignment": "map""#;
    let map_map = r#", "ngon-pitch-alignment": "map", "ngon-pitch-scale": "map""#;
    let viewport_map = r#", "ngon-pitch-scale": "map""#;
    let translate = r#", "ngon-translate": [20, -10], "ngon-translate-anchor": "viewport""#;
    // One behind the hill, one in front of it and one on its side.
    let around_hill = points(&[[0.0, 0.007], [-0.006, -0.006], [0.006, 0.0]]);
    let output = mln_headless::test_output_dir(env!("CARGO_TARGET_TMPDIR")).join("ngon_query_coverage");
    render_test::coverage::assert_matches(
        &builder.build(),
        &output,
        vec![
            ("ngon-flat", scene(FLAT, "mercator", None, &near, &ngon(""))),
            ("ngon-pitched", scene(PITCHED, "mercator", None, &near, &ngon(""))),
            ("ngon-pitched-map", scene(PITCHED, "mercator", None, &near, &ngon(map))),
            ("ngon-pitched-map-map", scene(PITCHED, "mercator", None, &near, &ngon(map_map))),
            ("ngon-pitched-viewport-map", scene(PITCHED, "mercator", None, &near, &ngon(viewport_map))),
            ("ngon-translate", scene(PITCHED, "mercator", None, &near, &ngon(translate))),
            ("ngon-globe", scene(GLOBE, "globe", None, &far, &ngon(""))),
            ("ngon-globe-map", scene(GLOBE, "globe", None, &far, &ngon(map))),
            ("ngon-horizon", scene(HORIZON, "globe", None, &points(&[[-8.0, 0.0], [-4.0, 15.0]]), &ngon(""))),
            ("ngon-terrain", scene(TERRAIN, "mercator", Some(hill()), &around_hill, &ngon(""))),
            ("ngon-terrain-map", scene(TERRAIN, "mercator", Some(hill()), &around_hill, &ngon(map))),
        ],
    );
}
