//! The `org.maplibre.location-puck` layer type through the facade, as an
//! app drives it: it sets the layer's paint properties as fixes come in,
//! and the puck moves and turns to them over the style's transition.

mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use mln_location_puck::LocationPuckLayer;
use mln_rs::mln_render::{Registry, RegistryBuilder};
use support::{Fixture, SIZE, wait};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// A puck a degree west of the center of a black map at zoom 3, heading
/// 350, whose properties transition over a second.
const STYLE: &str = r##"{"version":8,"zoom":3,"transition":{"duration":1000,"delay":0},"sources":{},
    "layers":[{"id":"background","type":"background","paint":{"background-color":"#000000"}},
        {"id":"puck","type":"org.maplibre.location-puck",
            "paint":{"position":[-1,0],"bearing":350,"bearing-visible":true}}]}"##;

fn registry() -> Arc<Registry> {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder).unwrap();
    mln_location_puck::register(&mut builder).unwrap();
    builder.build()
}

/// The puck's longitude and bearing as the last frame drew them.
fn drawn(fixture: &Fixture) -> (f64, f64) {
    let read = fixture.session.query_rendered_layer::<LocationPuckLayer, _>("puck", |_, values| {
        let paint = &values.evaluated.paint;
        Ok((paint.position.uniform().longitude(), paint.bearing.uniform().degrees()))
    });
    wait(read)
}

/// The premultiplied RGBA of the last frame's pixel `(x, y)`.
fn pixel(fixture: &Fixture, x: f64, y: f64) -> [u8; 4] {
    let image = wait(fixture.session.read_texture());
    let offset = ((y.round() as u32 * SIZE.width + x.round() as u32) * 4) as usize;
    image.data[offset..offset + 4].try_into().unwrap()
}

#[test]
fn a_fix_moves_the_puck_and_turns_its_arrow_through_north() {
    let fixture = Fixture::new(registry(), STYLE);
    assert_eq!((-1.0, 350.0), drawn(&fixture));

    wait(fixture.map.set_style_layer_property("puck", "position", "[1, 0]"));
    wait(fixture.map.set_style_layer_property("puck", "bearing", "10"));
    let deadline = Instant::now() + Duration::from_secs(30);
    let (mut midway, mut last_longitude) = (0, -1.0);
    loop {
        assert!(Instant::now() < deadline, "the transition never ended");
        fixture.render();
        let (longitude, bearing) = drawn(&fixture);
        assert!((last_longitude..=1.0).contains(&longitude), "{last_longitude} then {longitude}");
        assert!(!(10.0..350.0).contains(&bearing) || bearing == 10.0, "the long way round: {bearing}");
        last_longitude = longitude;
        if longitude == 1.0 && bearing == 10.0 {
            break;
        }
        if longitude > -1.0 && bearing != 350.0 {
            midway += 1;
            // The arrow points about north from the puck, which zoom 3 draws
            // 11.4 pixels a degree east of the center.
            let x = f64::from(SIZE.width) / 2.0 + longitude * 4096.0 / 360.0;
            let y = f64::from(SIZE.height) / 2.0;
            assert_eq!(WHITE, pixel(&fixture, x, y - 12.0), "north of the puck, heading {bearing}");
            assert_eq!(BLACK, pixel(&fixture, x, y + 14.0), "south of the puck, heading {bearing}");
        }
    }
    assert!(midway > 0, "no frame drew the transition under way");
}

#[test]
fn a_query_finds_the_puck_where_its_filter_takes_it() {
    use mln_rs::mln_style::expression::Expr;
    use mln_rs::mln_style::expression::value::Value;
    use mln_rs::mln_style::filter::Filter;

    let fixture = Fixture::new(registry(), STYLE);
    let found = |take: bool| {
        // The puck, a degree west of the center at zoom 3.
        let x = f64::from(SIZE.width) / 2.0 - 4096.0 / 360.0;
        let at = mln_rs::mln_util::geo::ScreenCoordinate::new(x, f64::from(SIZE.height) / 2.0);
        let filter = Filter::new(Some(Arc::new(Expr::literal(Value::Bool(take)))), None);
        let options = mln_rs::RenderedQueryOptions::new(Some(vec!["puck".to_owned()]), Some(filter));
        let features = wait(fixture.session.query_rendered_features(mln_rs::RenderedQueryGeometry::Point(at), options));
        features.into_iter().map(|feature| feature.layer).collect::<Vec<_>>()
    };
    assert_eq!(found(true), ["puck"]);
    assert_eq!(found(false), Vec::<String>::new());
}
