// C++ keys the indices on `util::hash(text, x, y)` (a `size_t`); here the
// key is the `(text, x, y)` tuple itself, which gives the same lookups
// except where two C++ (libstdc++ `std::hash` + `hash_combine`) hashes
// collide; C++ would then merge unrelated lines. The
// `mergeFromRight`/`mergeFromLeft` helpers take the index of the feature
// whose geometry C++ passes by reference, and the key the C++ iterator
// points at.

use std::collections::HashMap;

use mln_tile_id::geometry_tile_data::GeometryCoordinate;

use crate::layout::symbol_feature::SymbolFeature;

type Key = (Vec<u16>, i16, i16);

/// Map of key -> index into features
type Index = HashMap<Key, usize>;

fn get_key(text: &[u16], coord: GeometryCoordinate) -> Key {
    (text.to_vec(), coord.x, coord.y)
}

/// Appends `features[geom].geometry[0]` (minus the shared point) to the
/// feature `left` points at.
fn merge_from_right(
    features: &mut [SymbolFeature],
    right_index: &mut Index,
    left: &Key,
    right_key: Key,
    geom: usize,
) -> usize {
    let index = right_index[left];
    right_index.remove(left);
    right_index.insert(right_key, index);
    let geom0 = std::mem::take(&mut features[geom].geometry[0]);
    let target = &mut features[index].geometry[0];
    target.pop();
    target.extend_from_slice(&geom0);
    // `geom[0].clear()`: `geom0` was taken, leaving it empty.
    index
}

/// Prepends `features[geom].geometry[0]` (minus the shared point) to the
/// feature `right` points at.
fn merge_from_left(
    features: &mut [SymbolFeature],
    left_index: &mut Index,
    right: &Key,
    left_key: Key,
    geom: usize,
) -> usize {
    let index = left_index[right];
    left_index.remove(right);
    left_index.insert(left_key, index);
    let mut geom0 = std::mem::take(&mut features[geom].geometry[0]);
    geom0.pop();
    let mut other = std::mem::take(&mut features[index].geometry[0]);
    geom0.extend_from_slice(&other);
    other.clear();
    // `std::swap(features[index].geometry[0], geom[0])`.
    features[index].geometry[0] = geom0;
    features[geom].geometry[0] = other;
    index
}

pub(crate) fn merge_lines(features: &mut [SymbolFeature]) {
    let mut left_index = Index::new();
    let mut right_index = Index::new();

    for k in 0..features.len() {
        let feature = &features[k];
        let geometry = &feature.geometry;

        let Some(formatted_text) = &feature.formatted_text else {
            continue;
        };
        if geometry.is_empty() || geometry[0].is_empty() {
            continue;
        }

        // TODO: Key should include formatting options (see
        // https://github.com/mapbox/mapbox-gl-js/issues/3645)

        let text = formatted_text.raw_text().to_vec();
        let left_key = get_key(&text, *geometry[0].first().expect("not empty"));
        let right_key = get_key(&text, *geometry[0].last().expect("not empty"));

        let left = right_index.get(&left_key).copied();
        let right = left_index.get(&right_key).copied();

        match (left, right) {
            (Some(left), Some(right)) if left != right => {
                // found lines with the same text adjacent to both ends of the
                // current line, merge all three
                let j = merge_from_left(features, &mut left_index, &right_key, left_key.clone(), k);
                let i = merge_from_right(features, &mut right_index, &left_key, right_key.clone(), j);

                left_index.remove(&left_key);
                right_index.remove(&right_key);
                let back = *features[i].geometry[0].last().expect("merged line is not empty");
                right_index.insert(get_key(&text, back), i);
            }
            (Some(_), _) => {
                // found mergeable line adjacent to the start of the current line, merge
                merge_from_right(features, &mut right_index, &left_key, right_key, k);
            }
            (None, Some(_)) => {
                // found mergeable line adjacent to the end of the current line, merge
                merge_from_left(features, &mut left_index, &right_key, left_key, k);
            }
            (None, None) => {
                // no adjacent lines, add as a new item
                left_index.insert(left_key, k);
                right_index.insert(right_key, k);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use feature::Value;
    use geometry::Point;
    use mln_text::glyph::GlyphIDType;
    use mln_text::{SectionOptions, TaggedString};
    use mln_tile_id::geometry_tile_data::{GeometryCollection, GeometryCoordinates, GeometryTileFeature};
    use mln_util::geometry::FeatureType;

    use super::*;

    struct StubGeometryTileFeature {
        geometry: GeometryCollection,
    }

    impl GeometryTileFeature for StubGeometryTileFeature {
        fn get_type(&self) -> FeatureType {
            FeatureType::LineString
        }
        fn get_value(&self, _key: &str) -> Option<Value> {
            None
        }
        fn get_geometries(&self) -> &GeometryCollection {
            &self.geometry
        }
    }

    fn geometry(lines: &[&[(i16, i16)]]) -> GeometryCollection {
        GeometryCollection::from(
            lines
                .iter()
                .map(|line| line.iter().map(|&(x, y)| Point::new(x, y)).collect::<GeometryCoordinates>())
                .collect::<Vec<_>>(),
        )
    }

    fn symbol_feature_stub(geometry: GeometryCollection, text: &str) -> SymbolFeature {
        let mut feature = SymbolFeature::new(&StubGeometryTileFeature { geometry });
        feature.formatted_text = Some(TaggedString::new(
            text.encode_utf16().collect(),
            SectionOptions::new(1.0, Vec::new(), GlyphIDType::FontPBF, 0, None),
        ));
        feature.icon = None;
        feature.index = 0;
        feature
    }

    const AAA: &str = "a";
    const BBB: &str = "b";

    // gtest: MergeLines.SameText
    #[test]
    fn same_text() {
        // merges lines with the same text
        let mut input1 = vec![
            symbol_feature_stub(geometry(&[&[(0, 0), (1, 0), (2, 0)]]), AAA),
            symbol_feature_stub(geometry(&[&[(4, 0), (5, 0), (6, 0)]]), BBB),
            symbol_feature_stub(geometry(&[&[(8, 0), (9, 0)]]), AAA),
            symbol_feature_stub(geometry(&[&[(2, 0), (3, 0), (4, 0)]]), AAA),
            symbol_feature_stub(geometry(&[&[(6, 0), (7, 0), (8, 0)]]), AAA),
            symbol_feature_stub(geometry(&[&[(5, 0), (6, 0)]]), AAA),
        ];

        let expected1 = [
            geometry(&[&[(0, 0), (1, 0), (2, 0), (3, 0), (4, 0)]]),
            geometry(&[&[(4, 0), (5, 0), (6, 0)]]),
            geometry(&[&[(5, 0), (6, 0), (7, 0), (8, 0), (9, 0)]]),
            geometry(&[&[]]),
            geometry(&[&[]]),
            geometry(&[&[]]),
        ];

        merge_lines(&mut input1);

        for i in 0..6 {
            assert_eq!(input1[i].geometry, expected1[i]);
        }
    }

    // gtest: MergeLines.BothEnds
    #[test]
    fn both_ends() {
        // mergeLines handles merge from both ends
        let mut input2 = vec![
            symbol_feature_stub(geometry(&[&[(0, 0), (1, 0), (2, 0)]]), AAA),
            symbol_feature_stub(geometry(&[&[(4, 0), (5, 0), (6, 0)]]), AAA),
            symbol_feature_stub(geometry(&[&[(2, 0), (3, 0), (4, 0)]]), AAA),
        ];

        let expected2 = [
            geometry(&[&[(0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0)]]),
            geometry(&[&[]]),
            geometry(&[&[]]),
        ];

        merge_lines(&mut input2);

        for i in 0..3 {
            assert_eq!(input2[i].geometry, expected2[i]);
        }
    }

    // gtest: MergeLines.CircularLines
    #[test]
    fn circular_lines() {
        // mergeLines handles circular lines
        let mut input3 = vec![
            symbol_feature_stub(geometry(&[&[(0, 0), (1, 0), (2, 0)]]), AAA),
            symbol_feature_stub(geometry(&[&[(2, 0), (3, 0), (4, 0)]]), AAA),
            symbol_feature_stub(geometry(&[&[(4, 0), (0, 0)]]), AAA),
        ];

        let expected3 =
            [geometry(&[&[(0, 0), (1, 0), (2, 0), (3, 0), (4, 0), (0, 0)]]), geometry(&[&[]]), geometry(&[&[]])];

        merge_lines(&mut input3);

        for i in 0..3 {
            assert_eq!(input3[i].geometry, expected3[i]);
        }
    }

    // gtest: MergeLines.EmptyOuterGeometry
    #[test]
    fn empty_outer_geometry() {
        let mut input = vec![symbol_feature_stub(GeometryCollection::new(), AAA)];

        let expected = GeometryCollection::new();

        merge_lines(&mut input);

        assert_eq!(input[0].geometry, expected);
    }

    // gtest: MergeLines.EmptyInnerGeometry
    #[test]
    fn empty_inner_geometry() {
        let mut input = vec![symbol_feature_stub(GeometryCollection::new(), AAA)];

        let expected = GeometryCollection::new();

        merge_lines(&mut input);

        assert_eq!(input[0].geometry, expected);
    }
}
