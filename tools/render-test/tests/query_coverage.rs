//! A rendered-feature query finds each queryable built-in layer type where
//! it draws ([`render_test::coverage`]): flat, pitched, on the globe away
//! from the center's latitude and near the horizon, over terrain where a
//! hill hides some of it, aligned with the map or the viewport, and
//! translated.

use mln_engine::geo::ScreenCoordinate;
use render_test::coverage::{
    COLOR, FLAT, GLOBE, GLOBE_NEAR, HORIZON, MARK, PITCHED, SIDE, Scene, TERRAIN, hill, layer, line, points, scene,
    shows_at, square,
};

fn assert_matches(cases: Vec<(&str, Scene)>) {
    let output = mln_headless::test_output_dir(env!("CARGO_TARGET_TMPDIR")).join("query_coverage");
    render_test::coverage::assert_matches(&mln_builtins::registry(), &output, cases);
}

/// Whether a query of the box from `min` to `max`, in the view's pixels,
/// finds the layer of `scene`, which shows at `shown`, a pixel in the box,
/// where given; `None` where the machine has no device.
fn box_finds(scene: &Scene, [x0, y0]: [f64; 2], [x1, y1]: [f64; 2], shown: Option<[u32; 2]>) -> Option<bool> {
    let ring = [[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]].map(|[x, y]| ScreenCoordinate::new(x, y));
    let (found, image) = render_test::coverage::query(mln_builtins::registry(), scene, &ring)?;
    if let Some([x, y]) = shown {
        assert!(shows_at(&image, x, y), "the layer shows at {x}, {y}");
    }
    Some(found)
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
        // Past the source's maximum zoom, the caps that the tiles' zoom
        // lays out.
        (
            "line-cap-overscaled",
            scene(
                FLAT,
                "mercator",
                None,
                &near,
                &layer(
                    "line",
                    &format!(r#""line-color": "{COLOR}", "line-width": 16"#),
                    r#""line-cap": ["step", ["zoom"], "butt", 9, "round"]"#,
                ),
            )
            .with_source_maxzoom(8),
        ),
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
        // Lower than the planet's mesh rises above a roof's coarser one.
        ("fill-extrusion-globe-low", scene(GLOBE_NEAR, "globe", None, &square(0.0, 50.0, 3.0), &extrusion(1000.0))),
        (
            "fill-extrusion-terrain",
            scene(TERRAIN, "mercator", Some(hill()), &square(0.0, 0.007, 0.0015), &extrusion(800.0)),
        ),
    ]);
}

#[test]
fn a_box_query_finds_an_extrusion_whose_roof_it_crosses() {
    // A roof wide across the view and a narrow box across it, neither with
    // a corner inside the other.
    let roof = r#"{"type": "Polygon", "coordinates": [[[-0.06, -0.01], [0.06, -0.01], [0.06, 0.01], [-0.06, 0.01], [-0.06, -0.01]]]}"#;
    let extrusion =
        layer("fill-extrusion", &format!(r#""fill-extrusion-color": "{COLOR}", "fill-extrusion-height": 500"#), "");
    let scene = scene(FLAT, "mercator", None, roof, &extrusion);
    let middle = f64::from(SIDE / 2);
    let Some(across) = box_finds(&scene, [middle - 2.0, 64.0], [middle + 2.0, 192.0], Some([SIDE / 2, SIDE / 2]))
    else {
        return;
    };
    assert!(across, "a box across the roof finds it");
    let beside = box_finds(&scene, [230.0, 64.0], [240.0, 192.0], None);
    assert_eq!(beside, Some(false), "a box beside the roof doesn't find it");
}

#[test]
fn a_box_around_the_whole_globe_finds_circles_that_lie_on_it() {
    let circle = layer(
        "circle",
        &format!(r#""circle-color": "{COLOR}", "circle-radius": 6, "circle-pitch-alignment": "map""#),
        "",
    );
    let scene = scene([0.0, 0.0, 0.0, 0.0, 0.0], "globe", None, &points(&[[0.0, 0.0], [20.0, 10.0]]), &circle);
    let side = f64::from(SIDE);
    let middle = SIDE / 2;
    let found = box_finds(&scene, [1.0, 1.0], [side - 1.0, side - 1.0], Some([middle, middle]));
    assert_ne!(found, Some(false), "a box around the planet finds what it shows");
}
