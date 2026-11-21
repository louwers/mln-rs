//! The collision groups of a placement's sources.

use mln_vec_map::VecMap;

/// The collision groups of the sources: with cross-source collisions,
/// everything a placement places collides with everything else; without,
/// only with what its own source has. A placement makes them anew.
pub struct CollisionGroups {
    ids: VecMap<String, u16>,
    cross_source_collisions: bool,
}

impl CollisionGroups {
    /// The groups of a placement with `cross_source_collisions`, before it
    /// places anything.
    #[must_use]
    pub fn new(cross_source_collisions: bool) -> Self {
        CollisionGroups { ids: VecMap::new(), cross_source_collisions }
    }

    /// The group of what `source_id` has, which collides with that group
    /// alone; `None` where everything collides with everything, as group 0.
    pub fn get(&mut self, source_id: &str) -> Option<u16> {
        if self.cross_source_collisions {
            return None;
        }
        let next = self.ids.len() as u16 + 1;
        Some(*self.ids.get_or_insert_with(source_id, || next))
    }
}
