// Ports vendor/maplibre-native-base/deps/geojson-vt-cpp/include/mapbox/geojsonvt/clip.hpp.
//
// The C++ `clipper<I>` and `clip<I>` are templates on the axis `I`
// (0 = x, 1 = y); here a const generic.

use crate::types::{
    VtFeature, VtFeatures, VtGeometry, VtGeometryCollection, VtLineString, VtLinearRing, VtMultiLineString,
    VtMultiPoint, VtMultiPolygon, VtPolygon, calc_progress, get, get_point, intersect,
};

/// `clipper<I>`
pub(crate) struct Clipper<const I: u8> {
    pub(crate) k1: f64,
    pub(crate) k2: f64,
    pub(crate) line_metrics: bool,
}

impl<const I: u8> Clipper<I> {
    #[must_use]
    pub(crate) fn new(k1: f64, k2: f64, line_metrics: bool) -> Self {
        Clipper { k1, k2, line_metrics }
    }

    /// `operator()` on any `vt_geometry` alternative.
    #[must_use]
    pub(crate) fn clip(&self, geometry: &VtGeometry) -> VtGeometry {
        match geometry {
            VtGeometry::Empty => VtGeometry::Empty,
            VtGeometry::Point(point) => VtGeometry::Point(*point),
            VtGeometry::MultiPoint(points) => self.multi_point(points),
            VtGeometry::LineString(line) => self.line_string(line),
            VtGeometry::MultiLineString(lines) => self.multi_line_string(lines),
            VtGeometry::Polygon(polygon) => self.polygon(polygon),
            VtGeometry::MultiPolygon(polygons) => self.multi_polygon(polygons),
            VtGeometry::GeometryCollection(geometries) => self.geometry_collection(geometries),
        }
    }

    #[must_use]
    pub(crate) fn multi_point(&self, points: &VtMultiPoint) -> VtGeometry {
        let mut part = VtMultiPoint::new();
        for p in points {
            let ak = get::<I>(p);
            if ak >= self.k1 && ak <= self.k2 {
                part.push(*p);
            }
        }
        VtGeometry::MultiPoint(part)
    }

    #[must_use]
    pub(crate) fn line_string(&self, line: &VtLineString) -> VtGeometry {
        let mut parts = VtMultiLineString::new();
        self.clip_line(line, &mut parts);
        if parts.len() == 1 { VtGeometry::LineString(parts.swap_remove(0)) } else { VtGeometry::MultiLineString(parts) }
    }

    #[must_use]
    pub(crate) fn multi_line_string(&self, lines: &VtMultiLineString) -> VtGeometry {
        let mut parts = VtMultiLineString::new();
        for line in lines {
            self.clip_line(line, &mut parts);
        }
        if parts.len() == 1 { VtGeometry::LineString(parts.swap_remove(0)) } else { VtGeometry::MultiLineString(parts) }
    }

    #[must_use]
    pub(crate) fn polygon(&self, polygon: &VtPolygon) -> VtGeometry {
        let mut result = VtPolygon::new();
        for ring in polygon {
            let new_ring = self.clip_ring(ring);
            if !new_ring.is_empty() {
                result.push(new_ring);
            }
        }
        VtGeometry::Polygon(result)
    }

    #[must_use]
    pub(crate) fn multi_polygon(&self, polygons: &VtMultiPolygon) -> VtGeometry {
        let mut result = VtMultiPolygon::new();
        for polygon in polygons {
            let mut p = VtPolygon::new();
            for ring in polygon {
                let new_ring = self.clip_ring(ring);
                if !new_ring.is_empty() {
                    p.push(new_ring);
                }
            }
            if !p.is_empty() {
                result.push(p);
            }
        }
        VtGeometry::MultiPolygon(result)
    }

    #[must_use]
    pub(crate) fn geometry_collection(&self, geometries: &VtGeometryCollection) -> VtGeometry {
        let mut result = VtGeometryCollection::new();
        for geometry in geometries {
            result.push(self.clip(geometry));
        }
        VtGeometry::GeometryCollection(result)
    }

    fn new_slice(&self, line: &VtLineString) -> VtLineString {
        let mut slice = VtLineString { dist: line.dist, ..Default::default() };
        if self.line_metrics {
            slice.seg_start = line.seg_start;
            slice.seg_end = line.seg_end;
        }
        slice
    }

    fn clip_line(&self, line: &VtLineString, slices: &mut VtMultiLineString) {
        let (k1, k2, line_metrics) = (self.k1, self.k2, self.line_metrics);
        let len = line.len();
        let mut line_len = line.seg_start;
        let mut seg_len = 0.0;
        let mut t;

        if len < 2 {
            return;
        }

        let mut slice = self.new_slice(line);

        for i in 0..len - 1 {
            let a = &line[i];
            let b = &line[i + 1];
            let ak = get::<I>(a);
            let bk = get::<I>(b);
            let is_last_seg = i == len - 2;

            if line_metrics {
                seg_len = (b.x - a.x).hypot(b.y - a.y);
            }

            if ak < k1 {
                if bk > k2 {
                    // ---|-----|-->
                    t = calc_progress::<I>(a, b, k1);
                    slice.push(intersect::<I>(a, b, k1, t));
                    if line_metrics {
                        slice.seg_start = line_len + seg_len * t;
                    }

                    t = calc_progress::<I>(a, b, k2);
                    slice.push(intersect::<I>(a, b, k2, t));
                    if line_metrics {
                        slice.seg_end = line_len + seg_len * t;
                    }
                    slices.push(std::mem::replace(&mut slice, self.new_slice(line)));
                } else if bk > k1 {
                    // ---|-->  |
                    t = calc_progress::<I>(a, b, k1);
                    slice.push(intersect::<I>(a, b, k1, t));
                    if line_metrics {
                        slice.seg_start = line_len + seg_len * t;
                    }
                    if is_last_seg {
                        slice.push(*b); // last point
                    }
                } else if bk == k1 && !is_last_seg {
                    // --->|..  |
                    if line_metrics {
                        slice.seg_start = line_len + seg_len;
                    }
                    slice.push(*b);
                }
            } else if ak > k2 {
                if bk < k1 {
                    // <--|-----|---
                    t = calc_progress::<I>(a, b, k2);
                    slice.push(intersect::<I>(a, b, k2, t));
                    if line_metrics {
                        slice.seg_start = line_len + seg_len * t;
                    }

                    t = calc_progress::<I>(a, b, k1);
                    slice.push(intersect::<I>(a, b, k1, t));
                    if line_metrics {
                        slice.seg_end = line_len + seg_len * t;
                    }

                    slices.push(std::mem::replace(&mut slice, self.new_slice(line)));
                } else if bk < k2 {
                    // |  <--|---
                    t = calc_progress::<I>(a, b, k2);
                    slice.push(intersect::<I>(a, b, k2, t));
                    if line_metrics {
                        slice.seg_start = line_len + seg_len * t;
                    }
                    if is_last_seg {
                        slice.push(*b); // last point
                    }
                } else if bk == k2 && !is_last_seg {
                    // |  ..|<---
                    if line_metrics {
                        slice.seg_start = line_len + seg_len;
                    }
                    slice.push(*b);
                }
            } else {
                slice.push(*a);

                if bk < k1 {
                    // <--|---  |
                    t = calc_progress::<I>(a, b, k1);
                    slice.push(intersect::<I>(a, b, k1, t));
                    if line_metrics {
                        slice.seg_end = line_len + seg_len * t;
                    }
                    slices.push(std::mem::replace(&mut slice, self.new_slice(line)));
                } else if bk > k2 {
                    // |  ---|-->
                    t = calc_progress::<I>(a, b, k2);
                    slice.push(intersect::<I>(a, b, k2, t));
                    if line_metrics {
                        slice.seg_end = line_len + seg_len * t;
                    }
                    slices.push(std::mem::replace(&mut slice, self.new_slice(line)));
                } else if is_last_seg {
                    // | --> |
                    slice.push(*b);
                }
            }

            if line_metrics {
                line_len += seg_len;
            }
        }

        if !slice.is_empty() {
            // add the final slice
            if line_metrics {
                slice.seg_end = line_len;
            }
            slices.push(slice);
        }
    }

    fn clip_ring(&self, ring: &VtLinearRing) -> VtLinearRing {
        let (k1, k2) = (self.k1, self.k2);
        let len = ring.len();
        let mut slice = VtLinearRing { points: Vec::new(), area: ring.area };

        if len < 2 {
            return slice;
        }

        for i in 0..len - 1 {
            let a = &ring[i];
            let b = &ring[i + 1];
            let ak = get::<I>(a);
            let bk = get::<I>(b);

            if ak < k1 {
                if bk > k1 {
                    // ---|-->  |
                    slice.push(intersect::<I>(a, b, k1, calc_progress::<I>(a, b, k1)));
                    if bk > k2 {
                        // ---|-----|-->
                        slice.push(intersect::<I>(a, b, k2, calc_progress::<I>(a, b, k2)));
                    } else if i == len - 2 {
                        slice.push(*b); // last point
                    }
                }
            } else if ak > k2 {
                if bk < k2 {
                    // |  <--|---
                    slice.push(intersect::<I>(a, b, k2, calc_progress::<I>(a, b, k2)));
                    if bk < k1 {
                        // <--|-----|---
                        slice.push(intersect::<I>(a, b, k1, calc_progress::<I>(a, b, k1)));
                    } else if i == len - 2 {
                        slice.push(*b); // last point
                    }
                }
            } else {
                // | --> |
                slice.push(*a);
                if bk < k1 {
                    // <--|---  |
                    slice.push(intersect::<I>(a, b, k1, calc_progress::<I>(a, b, k1)));
                } else if bk > k2 {
                    // |  ---|-->
                    slice.push(intersect::<I>(a, b, k2, calc_progress::<I>(a, b, k2)));
                }
            }
        }

        // close the polygon if its endpoints are not the same after clipping
        if let (Some(first), Some(last)) = (slice.first(), slice.last())
            && first != last
        {
            let first = *first;
            slice.push(first);
        }

        slice
    }
}

/// `clip<I>(features, k1, k2, minAll, maxAll, lineMetrics)`: clip features
/// between two axis-parallel lines.
#[must_use]
pub(crate) fn clip<const I: u8>(
    features: &VtFeatures,
    k1: f64,
    k2: f64,
    min_all: f64,
    max_all: f64,
    line_metrics: bool,
) -> VtFeatures {
    if min_all >= k1 && max_all < k2 {
        // trivial accept
        return features.clone();
    }

    if max_all < k1 || min_all >= k2 {
        // trivial reject
        return VtFeatures::new();
    }

    let mut clipped = VtFeatures::with_capacity(features.len());

    for feature in features {
        let geom = &feature.geometry;
        let props = &feature.properties;
        let id = &feature.id;

        let min = get_point::<I>(&feature.bbox.min);
        let max = get_point::<I>(&feature.bbox.max);

        if min >= k1 && max < k2 {
            // trivial accept
            clipped.push(feature.clone());
        } else if max < k1 || min >= k2 {
            // trivial reject
        } else {
            let clipped_geom = Clipper::<I>::new(k1, k2, line_metrics).clip(geom);

            match clipped_geom {
                VtGeometry::MultiLineString(result) if line_metrics => {
                    for segment in result {
                        clipped.push(VtFeature::new(
                            VtGeometry::LineString(segment),
                            std::sync::Arc::clone(props),
                            id.clone(),
                        ));
                    }
                }
                clipped_geom => {
                    clipped.push(VtFeature::new(clipped_geom, std::sync::Arc::clone(props), id.clone()));
                }
            }
        }
    }

    clipped
}
