//! Port-only: hash maps and sets written in key order (serialization/mod.rs),
//! so that the same value always encodes to the same bytes.

/// `#[serde(with)]` for a `HashMap` with ordered keys.
pub mod map {
    use std::collections::HashMap;
    use std::hash::Hash;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<K, V, S>(map: &HashMap<K, V>, serializer: S) -> Result<S::Ok, S::Error>
    where
        K: Ord + Serialize,
        V: Serialize,
        S: Serializer,
    {
        let mut entries: Vec<(&K, &V)> = map.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        serializer.collect_map(entries)
    }

    pub fn deserialize<'de, K, V, D>(deserializer: D) -> Result<HashMap<K, V>, D::Error>
    where
        K: Eq + Hash + Deserialize<'de>,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        HashMap::deserialize(deserializer)
    }
}

/// `#[serde(with)]` for a `HashSet` of ordered values.
pub mod set {
    use std::collections::HashSet;
    use std::hash::Hash;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Ord + Serialize, S: Serializer>(set: &HashSet<T>, serializer: S) -> Result<S::Ok, S::Error> {
        let mut values: Vec<&T> = set.iter().collect();
        values.sort();
        serializer.collect_seq(values)
    }

    pub fn deserialize<'de, T, D>(deserializer: D) -> Result<HashSet<T>, D::Error>
    where
        T: Eq + Hash + Deserialize<'de>,
        D: Deserializer<'de>,
    {
        HashSet::deserialize(deserializer)
    }
}
