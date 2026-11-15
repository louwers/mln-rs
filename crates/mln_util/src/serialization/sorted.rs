//! Port-only: hash maps and sets written in key order (serialization/mod.rs),
//! so that the same value always encodes to the same bytes.

/// `#[wire(with)]` for a `HashMap` with ordered keys.
pub mod map {
    use std::hash::Hash;

    use crate::hash::{HashMap, RandomState};

    /// Writes the entries in key order. Keys are unique, so an unstable sort
    /// gives the same order.
    pub fn write<K: Ord + mln_wire::Wire, V: mln_wire::Wire>(map: &HashMap<K, V>, w: &mut mln_wire::Writer) {
        let mut entries: Vec<(&K, &V)> = map.iter().collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        mln_wire::Wire::write(&entries.len(), w);
        for (k, v) in entries {
            k.write(w);
            v.write(w);
        }
    }

    pub fn read<K, V>(r: &mut mln_wire::Reader<'_>) -> Option<HashMap<K, V>>
    where
        K: Eq + Hash + mln_wire::Wire,
        V: mln_wire::Wire,
    {
        let len = r.length()?;
        let mut map = HashMap::with_capacity_and_hasher(len.min(r.rest().len()), RandomState::default());
        for _ in 0..len {
            let k = K::read(r)?;
            map.insert(k, V::read(r)?);
        }
        Some(map)
    }
}

/// `#[wire(with)]` for a `HashSet` of ordered values.
pub mod set {
    use std::hash::Hash;

    use crate::hash::{HashSet, RandomState};

    /// Writes the values in order.
    pub fn write<T: Ord + mln_wire::Wire>(set: &HashSet<T>, w: &mut mln_wire::Writer) {
        let mut values: Vec<&T> = set.iter().collect();
        values.sort_unstable();
        mln_wire::Wire::write(&values.len(), w);
        for value in values {
            value.write(w);
        }
    }

    pub fn read<T: Eq + Hash + mln_wire::Wire>(r: &mut mln_wire::Reader<'_>) -> Option<HashSet<T>> {
        let len = r.length()?;
        let mut set = HashSet::with_capacity_and_hasher(len.min(r.rest().len()), RandomState::default());
        for _ in 0..len {
            set.insert(T::read(r)?);
        }
        Some(set)
    }
}
