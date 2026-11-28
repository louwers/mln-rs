//! A map whose size changes renders as a map made at the new size.

use mln_runtime::LogicalExtent;
use mln_style::bitmap::PremultipliedImage;
use mln_style::size::Size;

use crate::support::still::{StillMap, StillOptions};
use crate::support::wait;

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

/// A still map of the heatmap, made at `size`.
fn heatmap_map(size: Size) -> StillMap {
    let map = StillMap::new(StillOptions { size, ..StillOptions::default() });
    map.load_json(&heatmap_style());
    map
}

fn assert_same(actual: &PremultipliedImage, expected: &PremultipliedImage) {
    assert_eq!(actual.size, expected.size);
    let differing = actual.data.chunks(4).zip(expected.data.chunks(4)).filter(|(a, b)| a != b).count();
    assert_eq!(differing, 0, "{differing} pixels differ");
}

/// A heatmap whose session resizes to each size in turn, and back to the
/// first, renders as a map made at that size does.
#[test]
fn a_heatmap_whose_map_is_resized_renders_as_one_made_at_that_size() {
    let sizes = [Size::new(256, 256), Size::new(512, 384), Size::new(160, 200)];
    let made_at: Vec<_> = sizes.iter().map(|&size| heatmap_map(size).render()).collect();

    let map = heatmap_map(sizes[0]);
    for (&size, expected) in sizes.iter().zip(&made_at).chain([(&sizes[0], &made_at[0])]) {
        wait(map.session.resize(LogicalExtent { size, scale_factor: 1.0 }));
        assert_same(&map.render(), expected);
    }
}
