//! Port-only: hash maps and sets written in key order, so that the same
//! value always encodes to the same bytes.

#![expect(clippy::disallowed_types, reason = "maps and sets of every hasher, mln_style's among them")]

/// `#[wire(with)]` for a `HashMap` with ordered keys.
pub mod map {
    use std::collections::HashMap;
    use std::hash::{BuildHasher, Hash};

    /// Writes the entries in key order. Keys are unique, so an unstable sort
    /// gives the same order.
    pub fn write<K: Ord + crate::Wire, V: crate::Wire, S>(map: &HashMap<K, V, S>, w: &mut crate::Writer) {
        let mut entries: Vec<(&K, &V)> = map.iter().collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        crate::Wire::write(&entries.len(), w);
        for (k, v) in entries {
            k.write(w);
            v.write(w);
        }
    }

    pub fn read<K, V, S>(r: &mut crate::Reader<'_>) -> Option<HashMap<K, V, S>>
    where
        K: Eq + Hash + crate::Wire,
        V: crate::Wire,
        S: BuildHasher + Default,
    {
        let len = r.length()?;
        let mut map = HashMap::with_capacity_and_hasher(len.min(r.rest().len()), S::default());
        for _ in 0..len {
            let k = K::read(r)?;
            map.insert(k, V::read(r)?);
        }
        Some(map)
    }
}

/// `#[wire(with)]` for a `HashSet` of ordered values.
pub mod set {
    use std::collections::HashSet;
    use std::hash::{BuildHasher, Hash};

    /// Writes the values in order.
    pub fn write<T: Ord + crate::Wire, S>(set: &HashSet<T, S>, w: &mut crate::Writer) {
        let mut values: Vec<&T> = set.iter().collect();
        values.sort_unstable();
        crate::Wire::write(&values.len(), w);
        for value in values {
            value.write(w);
        }
    }

    pub fn read<T, S>(r: &mut crate::Reader<'_>) -> Option<HashSet<T, S>>
    where
        T: Eq + Hash + crate::Wire,
        S: BuildHasher + Default,
    {
        let len = r.length()?;
        let mut set = HashSet::with_capacity_and_hasher(len.min(r.rest().len()), S::default());
        for _ in 0..len {
            set.insert(T::read(r)?);
        }
        Some(set)
    }
}
