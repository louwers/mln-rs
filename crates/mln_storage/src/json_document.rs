// The JSON object and C's `strtol` as the MBTiles and PMTiles file sources
// use them to build TileJSON (platform/default/src/mln/storage/
// mbtiles_file_source.cpp, pmtiles_file_source.cpp). Not in
// mbtiles_file_source.rs, which needs the `sqlite` feature.

use serde_json::Value as Json;

/// A rapidjson `Document` object: members in insertion order, duplicate
/// names allowed (`AddMember` appends, `operator[]` finds the first).
#[derive(Default)]
pub(crate) struct Document {
    pub members: Vec<(String, Json)>,
}

impl Document {
    /// `doc.Parse(json)` followed by `if (!doc.IsObject()) doc.SetObject()`.
    pub fn parse_object(json: &[u8]) -> Self {
        // `Parse(const char*)` stops at the first NUL.
        let end = json.iter().position(|&b| b == 0).unwrap_or(json.len());
        match serde_json::from_slice::<Json>(&json[..end]) {
            Ok(Json::Object(object)) => Document { members: object.into_iter().collect() },
            _ => Document::default(),
        }
    }

    pub fn has_member(&self, name: &str) -> bool {
        self.members.iter().any(|(n, _)| n == name)
    }

    pub fn add_member(&mut self, name: impl Into<String>, value: Json) {
        self.members.push((name.into(), value));
    }

    /// `doc[name]` (the member must exist).
    pub fn get_mut(&mut self, name: &str) -> &mut Json {
        &mut self.members.iter_mut().find(|(n, _)| n == name).expect("member").1
    }

    /// The object as JSON. A name that appears more than once is written
    /// once, where it first appears, with the value added last.
    pub fn serialize(&self) -> String {
        Json::Object(self.members.iter().cloned().collect()).to_string()
    }
}

/// `std::stoi(s)` / `std::strtol(s, nullptr, 10)`: the leading integer;
/// 0 when there is none (C++ `stoi` throws then).
pub(crate) fn strtol(s: &str) -> i64 {
    let s = s.trim_start();
    let bytes = s.as_bytes();
    let mut end = 0;
    if end < bytes.len() && (bytes[end] == b'-' || bytes[end] == b'+') {
        end += 1;
    }
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    s[..end].parse::<i64>().unwrap_or(0)
}
