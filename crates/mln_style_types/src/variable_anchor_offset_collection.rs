// Ports include/mln/style/variable_anchor_offset_collection.hpp +
// src/mln/style/variable_anchor_offset_collection.cpp, plus the
// `util::Interpolator<VariableAnchorOffsetCollection>` specialization from
// include/mln/util/interpolate.hpp (as `VariableAnchorOffsetCollection::interpolate`,
// because C++ throws on mismatched inputs).

use feature::Value;
use mln_util::enum_::Enum;
use serde::{Deserialize, Serialize};

use crate::types::SymbolAnchorType;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnchorOffsetPair {
    pub anchor_type: SymbolAnchorType,
    pub offset: [f32; 2],
}

impl AnchorOffsetPair {
    pub fn new(anchor_type: SymbolAnchorType, offset: [f32; 2]) -> Self {
        AnchorOffsetPair { anchor_type, offset }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VariableAnchorOffsetCollection {
    anchor_offsets: Vec<AnchorOffsetPair>,
}

impl VariableAnchorOffsetCollection {
    pub fn new(values: Vec<AnchorOffsetPair>) -> Self {
        VariableAnchorOffsetCollection { anchor_offsets: values }
    }

    pub fn get_offset_by_anchor(&self, anchor_type: &SymbolAnchorType) -> [f32; 2] {
        for entry in &self.anchor_offsets {
            if *anchor_type == entry.anchor_type {
                return entry.offset;
            }
        }

        [0.0, 0.0]
    }

    pub fn to_string(&self) -> String {
        let mut writer = rapidjson::Writer::new();
        writer.start_array();
        for entry in &self.anchor_offsets {
            writer.string(entry.anchor_type.to_string());
            writer.start_array();
            writer.double(entry.offset[0] as f64);
            writer.double(entry.offset[1] as f64);
            writer.end_array();
        }

        writer.end_array();
        writer.into_string()
    }

    pub fn serialize(&self) -> Value {
        let mut serialized = Vec::new();
        for entry in &self.anchor_offsets {
            serialized.push(Value::String(entry.anchor_type.to_string().to_owned()));
            let offset = vec![Value::Double(entry.offset[0] as f64), Value::Double(entry.offset[1] as f64)];
            serialized.push(Value::Array(offset));
        }
        Value::Array(serialized)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor_offsets.is_empty()
    }

    pub fn len(&self) -> usize {
        self.anchor_offsets.len()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, AnchorOffsetPair> {
        self.anchor_offsets.iter()
    }

    /// `Interpolator<VariableAnchorOffsetCollection>` (float t; C++ throws a
    /// `std::runtime_error` with these messages).
    pub fn interpolate(a: &Self, b: &Self, t: f32) -> Result<Self, String> {
        if a.len() != b.len() {
            return Err(format!(
                "Cannot interpolate values of different length. from: {}, to: {}",
                a.to_string(),
                b.to_string()
            ));
        }
        let mut offset_map = Vec::with_capacity(a.len());
        for index in 0..a.len() {
            let a_pair = &a[index];
            let b_pair = &b[index];
            if a_pair.anchor_type != b_pair.anchor_type {
                // Quirk kept: no separator between the index and "from".
                return Err(format!(
                    "Cannot interpolate values containing mismatched anchors. index: {}from: {}, to: {}",
                    index,
                    a_pair.anchor_type.to_string(),
                    b_pair.anchor_type.to_string()
                ));
            }
            let lerp = |x: f32, y: f32| x * (1.0_f32 - t) + y * t;
            let offset = [lerp(a_pair.offset[0], b_pair.offset[0]), lerp(a_pair.offset[1], b_pair.offset[1])];
            offset_map.push(AnchorOffsetPair::new(a_pair.anchor_type, offset));
        }

        Ok(VariableAnchorOffsetCollection::new(offset_map))
    }
}

impl std::ops::Index<usize> for VariableAnchorOffsetCollection {
    type Output = AnchorOffsetPair;
    fn index(&self, index: usize) -> &AnchorOffsetPair {
        &self.anchor_offsets[index]
    }
}

impl<'a> IntoIterator for &'a VariableAnchorOffsetCollection {
    type Item = &'a AnchorOffsetPair;
    type IntoIter = std::slice::Iter<'a, AnchorOffsetPair>;
    fn into_iter(self) -> Self::IntoIter {
        self.anchor_offsets.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_offset_by_anchor() {
        let c = VariableAnchorOffsetCollection::new(vec![
            AnchorOffsetPair::new(SymbolAnchorType::Top, [1.0, 2.0]),
            AnchorOffsetPair::new(SymbolAnchorType::Left, [3.0, 4.0]),
        ]);
        assert_eq!(c.get_offset_by_anchor(&SymbolAnchorType::Left), [3.0, 4.0]);
        assert_eq!(c.get_offset_by_anchor(&SymbolAnchorType::Bottom), [0.0, 0.0]);
        assert_eq!(c.to_string(), r#"["top",[1.0,2.0],"left",[3.0,4.0]]"#);
    }
}
