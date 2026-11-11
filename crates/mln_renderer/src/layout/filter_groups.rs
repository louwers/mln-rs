//! Port-only: evaluates a layer filter once per group of features it can't
//! tell apart, rather than once per feature.

use mln_style::filter::Filter;
use mln_tile_id::geometry_tile_data::{FeatureGroups, GeometryTileFeature, GeometryTileLayer};

/// A tile's feature groups ([`GeometryTileLayer::group_features`]), made on
/// first use and shared by the filters over the same source layer that read
/// the same keys.
#[derive(Default)]
pub struct FilterGroups {
    /// Per source layer and set of keys: its groups, or none when the layer
    /// gave none.
    sets: Vec<(String, Box<[String]>, Option<FeatureGroups>)>,
}

impl FilterGroups {
    /// Whether `filter` selects each feature of `layer`, the source layer
    /// `name`, with `passes` evaluating it for one feature. A filter that
    /// reads only property keys and the geometry type evaluates once per
    /// group. `None` if the filter must evaluate each feature: it reads
    /// more of a feature, the layer has no groups, or evaluating a group's
    /// first feature found it malformed.
    pub fn select(
        &mut self,
        name: &str,
        layer: &dyn GeometryTileLayer,
        filter: &Filter,
        passes: impl Fn(&dyn GeometryTileFeature) -> bool,
    ) -> Option<Vec<bool>> {
        let keys = filter.property_keys()?;
        let at = match self.sets.iter().position(|(n, k, _)| n == name && **k == *keys) {
            Some(at) => at,
            None => {
                self.sets.push((name.to_owned(), keys.into(), layer.group_features(keys)));
                self.sets.len() - 1
            }
        };
        let groups = self.sets[at].2.as_ref()?;
        let mut group_passes = Vec::with_capacity(groups.first.len());
        for &first in &groups.first {
            let feature = layer.get_feature(first).ok()?;
            group_passes.push(passes(&*feature));
            if feature.error().is_some() {
                return None;
            }
        }
        Some(groups.of_feature.iter().map(|&group| group_passes[group as usize]).collect())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use feature::Value;
    use mln_style::conversion::{Convertible, Error, convert};
    use mln_style::expression::{EvaluationContext, GlobalStateMap};
    use mln_tile::VectorMVTTileData;
    use mln_tile_id::geometry_tile_data::GeometryTileData;

    use super::*;

    fn varint(out: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            out.push((value as u8) | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }

    fn varint_field(out: &mut Vec<u8>, tag: u64, value: u64) {
        varint(out, tag << 3);
        varint(out, value);
    }

    fn bytes_field(out: &mut Vec<u8>, tag: u64, bytes: &[u8]) {
        varint(out, (tag << 3) | 2);
        varint(out, bytes.len() as u64);
        out.extend_from_slice(bytes);
    }

    fn encode_value(value: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        match value {
            Value::String(s) => bytes_field(&mut out, 1, s.as_bytes()),
            Value::Double(d) => {
                varint(&mut out, (3 << 3) | 1);
                out.extend_from_slice(&d.to_le_bytes());
            }
            Value::Int(i) => varint_field(&mut out, 6, ((i << 1) ^ (i >> 63)) as u64),
            Value::Uint(u) => varint_field(&mut out, 5, *u),
            Value::Bool(b) => varint_field(&mut out, 7, u64::from(*b)),
            _ => unreachable!("no other values in these tiles"),
        }
        out
    }

    /// A road network as one MVT layer "roads": few distinct values over
    /// many features, keys some features lack, a column of mixed types, a
    /// key unique to each feature, equal values at distinct value indices,
    /// a key twice in the key table and a feature with a key twice.
    fn roads() -> Vec<u8> {
        let classes = ["motorway", "primary", "minor", "path"];
        let mut keys: Vec<String> = Vec::new();
        let mut values: Vec<Vec<u8>> = Vec::new();
        let mut value_index: HashMap<Vec<u8>, u32> = HashMap::new();
        let mut features = Vec::new();
        for i in 0..400u64 {
            let mut tags: Vec<(&str, Value)> = vec![
                ("class", Value::String(classes[i as usize % 4].into())),
                ("layer", Value::Int(i as i64 % 5 - 2)),
                ("ramp", Value::Bool(i % 11 == 0)),
                ("mixed", if i % 2 == 0 { Value::Uint(1) } else { Value::String("1".into()) }),
                ("width", Value::Double((i % 3) as f64 + 1.5)),
                ("name", Value::String(format!("road {i}"))),
                ("prefix", Value::String("cl".into())),
            ];
            if i % 3 == 0 {
                tags.push(("brunnel", Value::String("bridge".into())));
            }
            if i % 13 == 0 {
                // A repeated key: the first tag wins.
                tags.insert(0, ("class", Value::String("ferry".into())));
            }
            let mut packed = Vec::new();
            for (n, (key, value)) in tags.iter().enumerate() {
                // "class" is in the key table twice; features alternate.
                let key_at = match keys.iter().enumerate().filter(|(_, k)| k == key).nth((i as usize + n) % 2) {
                    Some((at, _)) => at,
                    None => {
                        keys.push((*key).to_owned());
                        keys.len() - 1
                    }
                };
                let encoded = encode_value(value);
                let value_at = match value_index.get(&encoded) {
                    Some(&at) if i % 37 != 0 => at,
                    _ => {
                        values.push(encoded.clone());
                        let at = values.len() as u32 - 1;
                        value_index.entry(encoded).or_insert(at);
                        at
                    }
                };
                varint(&mut packed, key_at as u64);
                varint(&mut packed, u64::from(value_at));
            }
            let geom_type = [2u64, 2, 2, 3, 1, 0][i as usize % 6];
            let geometry: &[u8] = match geom_type {
                1 => &[9, 2, 2],
                2 => &[9, 0, 0, 10, 2, 2],
                3 => &[9, 0, 0, 26, 2, 0, 0, 2, 1, 1, 15],
                _ => &[],
            };
            let mut feature = Vec::new();
            varint_field(&mut feature, 1, i);
            bytes_field(&mut feature, 2, &packed);
            varint_field(&mut feature, 3, geom_type);
            bytes_field(&mut feature, 4, geometry);
            features.push(feature);
        }
        let mut layer = Vec::new();
        varint_field(&mut layer, 15, 2);
        bytes_field(&mut layer, 1, b"roads");
        for feature in &features {
            bytes_field(&mut layer, 2, feature);
        }
        for key in &keys {
            bytes_field(&mut layer, 3, key.as_bytes());
        }
        for value in &values {
            bytes_field(&mut layer, 4, value);
        }
        varint_field(&mut layer, 5, 4096);
        let mut tile = Vec::new();
        bytes_field(&mut tile, 3, &layer);
        tile
    }

    fn parse_filter(json: &str) -> Filter {
        let json: serde_json::Value = serde_json::from_str(json).unwrap();
        convert::<Filter>(&Convertible::from(&json), &mut Error::default()).expect("filter")
    }

    /// Filters select the same features in groups as one by one, at each
    /// zoom; those that read more of a feature evaluate each feature.
    #[test]
    fn grouped_selection_matches_each_feature() {
        let data = VectorMVTTileData::new(Arc::new(roads()));
        let layer = data.get_layer("roads").unwrap().unwrap();
        let layer = &*layer;
        let mut global_state = GlobalStateMap::new();
        global_state.insert("class".to_owned(), Value::String("primary".into()));
        let mut groups = FilterGroups::default();
        let grouped = [
            r#"["==", ["get", "class"], "motorway"]"#,
            r#"["==", ["get", "class"], "ferry"]"#,
            r#"["all", ["==", "$type", "LineString"], ["!=", "brunnel", "bridge"]]"#,
            r#"["match", ["get", "class"], ["primary", "minor"], ["<", ["get", "layer"], 0], false]"#,
            r#"["any", ["get", "ramp"], ["has", "brunnel"]]"#,
            r#"["!has", "brunnel"]"#,
            r#"["has", "nothing"]"#,
            r#"["in", "class", "path", "minor"]"#,
            r#"[">=", "layer", 0]"#,
            r#"[">", ["get", "width"], 2]"#,
            r#"["==", ["get", "mixed"], 1]"#,
            r#"["to-boolean", ["get", "mixed"]]"#,
            r#"["==", ["geometry-type"], "Polygon"]"#,
            r#"["in", "$type", "Point", "Polygon"]"#,
            r#"["==", ["get", ["concat", "cl", "ass"]], "path"]"#,
            r#"["all", [">=", ["zoom"], 12], ["==", ["get", "class"], "path"]]"#,
            r#"["step", ["zoom"], ["==", ["get", "class"], "minor"], 12, ["has", "brunnel"]]"#,
            r#"["==", ["get", "class"], ["global-state", "class"]]"#,
            r#"["let", "c", ["get", "class"], ["==", ["var", "c"], "minor"]]"#,
        ];
        let per_feature = [
            r#"["<", ["id"], 30]"#,
            r#"["has", "$id"]"#,
            r#"["==", ["get", ["concat", ["get", "prefix"], "ass"]], "path"]"#,
            r#"["==", ["get", "class", ["properties"]], "path"]"#,
            r#"["==", ["get", "name"], "road 7"]"#,
            r#"["within", {"type": "Polygon", "coordinates": [[[0, 0], [0, 1], [1, 1], [0, 0]]]}]"#,
        ];
        for (text, expect_grouped) in grouped.iter().map(|t| (t, true)).chain(per_feature.iter().map(|t| (t, false))) {
            let filter = parse_filter(text);
            for zoom in [10.0, 14.0] {
                let passes = |feature: &dyn GeometryTileFeature| {
                    filter.evaluate(
                        &EvaluationContext::with_zoom_feature(zoom, Some(feature))
                            .with_global_state(Some(&global_state)),
                    )
                };
                let one_by_one: Vec<bool> =
                    (0..layer.feature_count()).map(|i| passes(&*layer.get_feature(i).unwrap())).collect();
                let selects_none = ["nothing", "within", "zoom"].iter().any(|t| text.contains(t));
                assert!(one_by_one.contains(&true) || selects_none, "{text}");
                let selected = groups.select("roads", layer, &filter, passes);
                if expect_grouped {
                    assert_eq!(selected.as_ref(), Some(&one_by_one), "{text} at {zoom}");
                } else {
                    assert_eq!(selected, None, "{text}");
                }
            }
        }
        let groups_of = |keys: &[&str]| {
            let (_, _, groups) = (groups.sets.iter())
                .find(|(_, k, _)| k.iter().map(String::as_str).eq(keys.iter().copied()))
                .expect("grouped");
            groups.as_ref().map(|g| g.first.len())
        };
        // 400 roads in about 4 classes by 4 geometry types, plus the few
        // values at a second index.
        assert!(groups_of(&["class"]).is_some_and(|n| n < 50), "{:?}", groups_of(&["class"]));
        // A key unique to each feature gives no groups.
        assert_eq!(groups_of(&["name"]), None);
    }
}
