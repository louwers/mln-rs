//! A rendered-feature query finds each queryable built-in layer type where
//! it draws ([`render_test::coverage`]): flat, pitched, on the globe away
//! from the center's latitude and near the horizon, over terrain where a
//! hill hides some of it, aligned with the map or the viewport, and
//! translated.

use render_test::coverage::{
    COLOR, FLAT, GLOBE, HORIZON, MARK, PITCHED, Scene, TERRAIN, hill, layer, line, points, scene, square,
};

fn assert_matches(cases: Vec<(&str, Scene)>) {
    let output = mln_headless::test_output_dir(env!("CARGO_TARGET_TMPDIR")).join("query_coverage");
    render_test::coverage::assert_matches(&mln_builtins::registry(), &output, cases);
}

#[test]
fn a_query_finds_a_fill_where_it_draws() {
    let fill = |extra: &str| layer("fill", &format!(r#""fill-color": "{COLOR}", "fill-antialias": false{extra}"#), "");
    let near = square(0.0, 0.0, 0.03);
    let far = square(0.0, 60.0, 4.0);
    let translate = r#", "fill-translate": [30, -20], "fill-translate-anchor": "viewport""#;
    assert_matches(vec![
        ("fill-flat", scene(FLAT, "mercator", None, &near, &fill(""))),
        ("fill-pitched", scene(PITCHED, "mercator", None, &near, &fill(""))),
        ("fill-translate", scene(PITCHED, "mercator", None, &near, &fill(translate))),
        ("fill-globe", scene(GLOBE, "globe", None, &far, &fill(""))),
        ("fill-globe-translate", scene(GLOBE, "globe", None, &far, &fill(r#", "fill-translate": [20, 30]"#))),
        ("fill-horizon", scene(HORIZON, "globe", None, &square(5.0, 0.0, 25.0), &fill(""))),
        ("fill-terrain", scene(TERRAIN, "mercator", Some(hill()), &square(0.0, 0.004, 0.006), &fill(""))),
    ]);
}

#[test]
fn a_query_finds_a_line_where_it_draws() {
    let line_layer = |width: f64, extra: &str| {
        layer("line", &format!(r#""line-color": "{COLOR}", "line-width": {width}{extra}"#), "")
    };
    let wide = |extra: &str| line_layer(16.0, extra);
    let near = line(&[[-0.05, -0.02], [0.0, 0.03], [0.05, -0.01]]);
    let far = line(&[[-15.0, 58.0], [0.0, 63.0], [15.0, 57.0]]);
    let offset = r#", "line-offset": 10, "line-translate": [15, 10], "line-translate-anchor": "viewport""#;
    let over_hill = line(&[[-0.008, -0.002], [0.0, 0.004], [0.008, -0.001]]);
    assert_matches(vec![
        ("line-flat", scene(FLAT, "mercator", None, &near, &wide(""))),
        ("line-pitched", scene(PITCHED, "mercator", None, &near, &wide(""))),
        ("line-offset", scene(PITCHED, "mercator", None, &near, &wide(offset))),
        ("line-globe", scene(GLOBE, "globe", None, &far, &wide(""))),
        ("line-globe-offset", scene(GLOBE, "globe", None, &far, &wide(offset))),
        ("line-horizon", scene(HORIZON, "globe", None, &line(&[[-25.0, -20.0], [-20.0, 20.0]]), &wide(""))),
        ("line-terrain", scene(TERRAIN, "mercator", Some(hill()), &over_hill, &line_layer(30.0, ""))),
    ]);
}

#[test]
fn a_query_finds_a_circle_where_it_draws() {
    let circle = |extra: &str| {
        layer("circle", &format!(r#""circle-color": "{COLOR}", "circle-radius": 14, "circle-blur": 0{extra}"#), "")
    };
    let near = points(&[[-0.03, 0.0], [0.0, 0.02], [0.03, -0.01]]);
    let far = points(&[[-12.0, 58.0], [0.0, 62.0], [12.0, 57.0]]);
    let map = r#", "circle-pitch-alignment": "map""#;
    let map_map = r#", "circle-pitch-alignment": "map", "circle-pitch-scale": "map""#;
    let viewport_map = r#", "circle-pitch-scale": "map""#;
    let translate = r#", "circle-translate": [20, -10], "circle-translate-anchor": "viewport""#;
    // One behind the hill, one in front of it and one on its side.
    let around_hill = points(&[[0.0, 0.007], [-0.006, -0.006], [0.006, 0.0]]);
    assert_matches(vec![
        ("circle-flat", scene(FLAT, "mercator", None, &near, &circle(""))),
        ("circle-pitched", scene(PITCHED, "mercator", None, &near, &circle(""))),
        ("circle-pitched-map", scene(PITCHED, "mercator", None, &near, &circle(map))),
        ("circle-pitched-map-map", scene(PITCHED, "mercator", None, &near, &circle(map_map))),
        ("circle-pitched-viewport-map", scene(PITCHED, "mercator", None, &near, &circle(viewport_map))),
        ("circle-translate", scene(PITCHED, "mercator", None, &near, &circle(translate))),
        ("circle-globe", scene(GLOBE, "globe", None, &far, &circle(""))),
        ("circle-globe-map", scene(GLOBE, "globe", None, &far, &circle(map))),
        ("circle-globe-map-map", scene(GLOBE, "globe", None, &far, &circle(map_map))),
        ("circle-horizon", scene(HORIZON, "globe", None, &points(&[[-8.0, 0.0], [-4.0, 15.0]]), &circle(""))),
        ("circle-terrain", scene(TERRAIN, "mercator", Some(hill()), &around_hill, &circle(""))),
        ("circle-terrain-map", scene(TERRAIN, "mercator", Some(hill()), &around_hill, &circle(map))),
    ]);
}

#[test]
fn a_query_finds_an_icon_where_it_draws() {
    let icon = |extra: &str| {
        layer(
            "symbol",
            "",
            &format!(r#""icon-image": "{MARK}", "icon-allow-overlap": true, "icon-ignore-placement": true{extra}"#),
        )
    };
    let near = points(&[[-0.03, 0.0], [0.0, 0.02], [0.03, -0.01]]);
    let far = points(&[[-12.0, 58.0], [0.0, 62.0], [12.0, 57.0]]);
    let around_hill = points(&[[0.0, 0.007], [-0.006, -0.006], [0.006, 0.0]]);
    assert_matches(vec![
        ("icon-flat", scene(FLAT, "mercator", None, &near, &icon(""))),
        ("icon-pitched", scene(PITCHED, "mercator", None, &near, &icon(""))),
        ("icon-globe", scene(GLOBE, "globe", None, &far, &icon(""))),
        ("icon-horizon", scene(HORIZON, "globe", None, &points(&[[-8.0, 0.0], [-4.0, 15.0]]), &icon(""))),
        ("icon-terrain", scene(TERRAIN, "mercator", Some(hill()), &around_hill, &icon(""))),
    ]);
}

#[test]
fn a_query_finds_a_fill_extrusion_where_it_draws() {
    let extrusion = |height: f64| {
        layer("fill-extrusion", &format!(r#""fill-extrusion-color": "{COLOR}", "fill-extrusion-height": {height}"#), "")
    };
    let near = square(0.0, 0.0, 0.015);
    assert_matches(vec![
        ("fill-extrusion-flat", scene(FLAT, "mercator", None, &near, &extrusion(500.0))),
        ("fill-extrusion-pitched", scene(PITCHED, "mercator", None, &near, &extrusion(500.0))),
        ("fill-extrusion-globe", scene(GLOBE, "globe", None, &square(0.0, 60.0, 3.0), &extrusion(0.0))),
        (
            "fill-extrusion-terrain",
            scene(TERRAIN, "mercator", Some(hill()), &square(0.0, 0.007, 0.0015), &extrusion(800.0)),
        ),
    ]);
}
