// A map whose size changes renders as a map made at the new size.

mod support;

use mln_util::image::PremultipliedImage;
use mln_util::size::Size;
use support::*;

/// A heatmap over a grid of points that reaches every edge of the map: a
/// heatmap drawn at the wrong size shows as a cropped or shifted grid.
fn heatmap_style() -> String {
    let mut features = Vec::new();
    for x in -8..=8 {
        for y in -4..=4 {
            features.push(format!(
                r#"{{ "type": "Feature", "properties": {{}}, "geometry": {{ "type": "Point", "coordinates": [{}, {}] }} }}"#,
                x * 20,
                y * 15
            ));
        }
    }
    format!(
        r#"{{
  "version": 8,
  "center": [0, 0],
  "zoom": 0,
  "sources": {{
    "points": {{ "type": "geojson", "data": {{ "type": "FeatureCollection", "features": [{}] }} }}
  }},
  "layers": [
    {{ "id": "background", "type": "background", "paint": {{ "background-color": "white" }} }},
    {{ "id": "heatmap", "type": "heatmap", "source": "points",
      "paint": {{ "heatmap-radius": 12, "heatmap-intensity": 2 }} }}
  ]
}}"#,
        features.join(",")
    )
}

fn map_test(size: Size) -> MapTest {
    let test = MapTest::new();
    test.frontend.clone().set_size(size);
    test.map().set_size(size).unwrap();
    test.load_json(&heatmap_style());
    test
}

fn assert_same(actual: &PremultipliedImage, expected: &PremultipliedImage) {
    assert_eq!(actual.size, expected.size);
    let differing = actual.data.chunks(4).zip(expected.data.chunks(4)).filter(|(a, b)| a != b).count();
    assert_eq!(differing, 0, "{differing} pixels differ");
}

#[test]
fn a_heatmap_whose_map_is_resized_renders_as_one_made_at_that_size() {
    let sizes = [Size::new(256, 256), Size::new(512, 384), Size::new(160, 200)];
    // One map at a time: each has the thread's run loop.
    let made_at: Vec<_> = sizes.iter().map(|&size| map_test(size).render().image).collect();

    let test = map_test(sizes[0]);
    for (&size, expected) in sizes.iter().zip(&made_at).chain([(&sizes[0], &made_at[0])]) {
        test.frontend.clone().set_size(size);
        test.map().set_size(size).unwrap();
        assert_same(&test.render().image, expected);
    }
}
