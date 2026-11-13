// Ports include/mln/style/variable_anchor_offset_collection.hpp +
// src/mln/style/variable_anchor_offset_collection.cpp, plus the
// `util::Interpolator<VariableAnchorOffsetCollection>` specialization from
// include/mln/util/interpolate.hpp (as `VariableAnchorOffsetCollection::interpolate`,
// because C++ throws on mismatched inputs).

use feature::Value;
use mln_util::string::to_string_double;
use serde::{Deserialize, Serialize};

use crate::types::SymbolAnchorType;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnchorOffsetPair {
    pub anchor_type: SymbolAnchorType,
    pub offset: [f32; 2],
}

impl AnchorOffsetPair {
    #[must_use]
    pub fn new(anchor_type: SymbolAnchorType, offset: [f32; 2]) -> Self {
        AnchorOffsetPair { anchor_type, offset }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VariableAnchorOffsetCollection {
    anchor_offsets: Vec<AnchorOffsetPair>,
}

impl VariableAnchorOffsetCollection {
    #[must_use]
    pub fn new(values: Vec<AnchorOffsetPair>) -> Self {
        VariableAnchorOffsetCollection { anchor_offsets: values }
    }

    #[must_use]
    pub fn get_offset_by_anchor(&self, anchor_type: &SymbolAnchorType) -> [f32; 2] {
        for entry in &self.anchor_offsets {
            if *anchor_type == entry.anchor_type {
                return entry.offset;
            }
        }

        [0.0, 0.0]
    }

    #[must_use]
    pub fn serialize(&self) -> Value {
        let mut serialized = Vec::new();
        for entry in &self.anchor_offsets {
            serialized.push(Value::String(entry.anchor_type.as_str().to_owned()));
            let offset = vec![Value::Double(f64::from(entry.offset[0])), Value::Double(f64::from(entry.offset[1]))];
            serialized.push(Value::Array(offset));
        }
        Value::Array(serialized)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.anchor_offsets.is_empty()
    }

    #[must_use]
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
            return Err(format!("Cannot interpolate values of different length. from: {a}, to: {b}"));
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
                    a_pair.anchor_type.as_str(),
                    b_pair.anchor_type.as_str()
                ));
            }
            let lerp = |x: f32, y: f32| x * (1.0_f32 - t) + y * t;
            let offset = [lerp(a_pair.offset[0], b_pair.offset[0]), lerp(a_pair.offset[1], b_pair.offset[1])];
            offset_map.push(AnchorOffsetPair::new(a_pair.anchor_type, offset));
        }

        Ok(VariableAnchorOffsetCollection::new(offset_map))
    }
}

/// The collection as JSON, an array of anchor names each followed by its
/// offset, with JavaScript's number formatting (GL JS's `toString()`).
impl std::fmt::Display for VariableAnchorOffsetCollection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[")?;
        for (i, entry) in self.anchor_offsets.iter().enumerate() {
            let separator = if i == 0 { "" } else { "," };
            let [x, y] = entry.offset.map(|v| to_string_double(v.into()));
            write!(f, "{separator}\"{}\",[{x},{y}]", entry.anchor_type.as_str())?;
        }
        f.write_str("]")
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
        assert_eq!(c.to_string(), r#"["top",[1,2],"left",[3,4]]"#);
    }
}
