// The TileJSON that the MBTiles and PMTiles file sources answer a source
// request with.

use serde_json::{Map, Value};

/// The TileJSON of an archive: `members`, which the archive's tiles determine, then each member of
/// `metadata` that no earlier one names. Metadata members of the wrong type are dropped, and so are
/// those that only the tiles determine, such as `tiles` and the zoom range, whose metadata values
/// may be stale or written as strings.
pub(crate) fn archive_tilejson<'a>(
    members: impl IntoIterator<Item = (&'a str, Value)>,
    metadata: impl IntoIterator<Item = (String, Value)>,
) -> String {
    let mut tilejson = Map::new();
    tilejson.insert("tilejson".to_owned(), "3.0.0".into());
    tilejson.insert("scheme".to_owned(), "xyz".into());
    for (name, value) in members {
        tilejson.insert(name.to_owned(), value);
    }
    for (name, value) in metadata {
        let keep = match name.as_str() {
            "tilejson" | "tiles" | "scheme" | "minzoom" | "maxzoom" | "bounds" | "center" => false,
            "attribution" | "description" | "legend" | "name" | "version" => value.is_string(),
            "vector_layers" => value.is_array(),
            _ => true,
        };
        if keep {
            tilejson.entry(name).or_insert(value);
        }
    }
    Value::Object(tilejson).to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn keeps_the_archives_members_and_drops_mistyped_ones() {
        let Value::Object(metadata) = json!({
            "name": "metadata",
            "attribution": "© OpenStreetMap",
            "description": 7,
            "minzoom": "0",
            "tiles": ["https://elsewhere/{z}/{x}/{y}"],
            "vector_layers": [{ "id": "water", "fields": {} }],
            "type": "baselayer",
        }) else {
            unreachable!()
        };
        let members = [("tiles", json!(["pmtiles://a"])), ("minzoom", json!(2)), ("name", json!("archive"))];
        let tilejson: Value = serde_json::from_str(&archive_tilejson(members, metadata)).unwrap();
        assert_eq!(
            json!({
                "tilejson": "3.0.0",
                "scheme": "xyz",
                "tiles": ["pmtiles://a"],
                "minzoom": 2,
                "name": "archive",
                "attribution": "© OpenStreetMap",
                "vector_layers": [{ "id": "water", "fields": {} }],
                "type": "baselayer",
            }),
            tilejson
        );
    }
}
