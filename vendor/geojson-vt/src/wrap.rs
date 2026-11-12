// Ports vendor/maplibre-native-base/deps/geojson-vt-cpp/include/mapbox/geojsonvt/wrap.hpp.

use crate::clip::clip;
use crate::types::VtFeatures;

pub fn shift_coords(features: &mut VtFeatures, offset: f64) {
    for feature in features {
        feature.geometry.for_each_point_mut(&mut |point| point.x += offset);
        feature.bbox.min.x += offset;
        feature.bbox.max.x += offset;
    }
}

#[must_use]
pub fn wrap(features: &VtFeatures, buffer: f64, line_metrics: bool) -> VtFeatures {
    // left world copy
    let mut left = clip::<0>(features, -1.0 - buffer, buffer, -1.0, 2.0, line_metrics);
    // right world copy
    let mut right = clip::<0>(features, 1.0 - buffer, 2.0 + buffer, -1.0, 2.0, line_metrics);

    if left.is_empty() && right.is_empty() {
        return features.clone();
    }

    // center world copy
    let mut merged = clip::<0>(features, -buffer, 1.0 + buffer, -1.0, 2.0, line_metrics);

    if !left.is_empty() {
        // merge left into center
        shift_coords(&mut left, 1.0);
        left.append(&mut merged);
        merged = left;
    }
    if !right.is_empty() {
        // merge right into center
        shift_coords(&mut right, -1.0);
        merged.append(&mut right);
    }
    merged
}
