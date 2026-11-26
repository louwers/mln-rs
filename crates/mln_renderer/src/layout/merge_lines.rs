use mln_style::feature::GeometryCoordinate;
use mln_util::hash::HashMap;

use crate::layout::symbol_feature::SymbolFeature;

/// A line's end: the index of its feature's text among the distinct texts,
/// and the end point.
type End = (usize, i16, i16);

fn end(text: usize, point: GeometryCoordinate) -> End {
    (text, point.x, point.y)
}

/// The index of each feature's text among the distinct texts, if it has
/// one.
fn text_ids(features: &[SymbolFeature]) -> Vec<Option<usize>> {
    let mut ids: HashMap<&[char], usize> = HashMap::default();
    features
        .iter()
        .map(|feature| {
            let text = feature.formatted_text.as_ref()?.text();
            let next = ids.len();
            Some(*ids.entry(text).or_insert(next))
        })
        .collect()
}

/// The first line of `features[index]`.
fn first_line(features: &mut [SymbolFeature], index: usize) -> &mut Vec<GeometryCoordinate> {
    &mut features[index].geometry[0]
}

/// Moves the first line of `features[from]` onto the end of the one of
/// `features[to]`, which ends where it starts.
fn append(features: &mut [SymbolFeature], to: usize, from: usize) {
    let tail = std::mem::take(first_line(features, from));
    let line = first_line(features, to);
    line.pop();
    line.extend(tail);
}

/// Moves the first line of `features[from]` onto the start of the one of
/// `features[to]`, which starts where it ends.
fn prepend(features: &mut [SymbolFeature], to: usize, from: usize) {
    let mut head = std::mem::take(first_line(features, from));
    head.pop();
    first_line(features, to).splice(..0, head);
}

/// Joins the first lines of features with the same text where one ends and
/// another starts. The joined line takes the place of one of them, and the
/// others are left empty.
pub(crate) fn merge_lines(features: &mut [SymbolFeature]) {
    let text_ids = text_ids(features);
    // The features whose lines start, and end, at an end.
    let mut by_start: HashMap<End, usize> = HashMap::default();
    let mut by_end: HashMap<End, usize> = HashMap::default();

    for (k, text) in text_ids.into_iter().enumerate() {
        // TODO: Key should include formatting options (see
        // https://github.com/mapbox/mapbox-gl-js/issues/3645)
        let Some(text) = text else { continue };
        let Some(line) = features[k].geometry.first() else { continue };
        let (Some(&first), Some(&last)) = (line.first(), line.last()) else { continue };
        let (start, finish) = (end(text, first), end(text, last));

        match (by_end.get(&start).copied(), by_start.get(&finish).copied()) {
            (Some(before), Some(after)) if before != after => {
                // The line joins two others: join all three into the one
                // before.
                prepend(features, after, k);
                append(features, before, after);
                by_end.remove(&start);
                by_start.remove(&finish);
                if let Some(&last) = features[before].geometry[0].last() {
                    by_end.insert(end(text, last), before);
                }
            }
            (Some(before), _) => {
                append(features, before, k);
                by_end.remove(&start);
                by_end.insert(finish, before);
            }
            (None, Some(after)) => {
                prepend(features, after, k);
                by_start.remove(&finish);
                by_start.insert(start, after);
            }
            (None, None) => {
                by_start.insert(start, k);
                by_end.insert(finish, k);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use feature::Value;
    use geometry::Point;
    use mln_style::feature::{GeometryCollection, GeometryCoordinates, GeometryTileFeature};
    use mln_style::geometry::FeatureType;
    use mln_text::glyph::GlyphIDType;
    use mln_text::{SectionOptions, TaggedString};

    use super::*;

    struct StubGeometryTileFeature;

    impl GeometryTileFeature for StubGeometryTileFeature {
        fn get_type(&self) -> FeatureType {
            FeatureType::LineString
        }
        fn get_value(&self, _key: &str) -> Option<Value> {
            None
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
        let mut feature = SymbolFeature::new(&StubGeometryTileFeature, geometry);
        feature.formatted_text = Some(TaggedString::new(
            text.chars().collect(),
            SectionOptions::new(1.0, Vec::new(), GlyphIDType::FontPBF, None),
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
