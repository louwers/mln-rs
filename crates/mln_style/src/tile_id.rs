//! The canonical tile id: a tile of the tile pyramid, by zoom level and
//! column and row.

// Integer arithmetic follows the C++ types: `uint8_t`/`uint32_t` arithmetic
// wraps, and shifts are `wrapping_sh*` (x86 masks the shift count, which is
// what the C++ code compiles to where it would be UB).
//
// Ordering: the derived `Ord` compares fields in declaration order, which is
// the `std::tie` order of the C++ `operator<`.
//
// Hashing: `std::hash` specializations combine the fields with
// `util::hash_combine`; the port derives `Hash` (field order differs from the
// C++ combine order, but no ported code depends on hash values).

use std::fmt;

/// Has integer z/x/y coordinates. All tiles must be derived from 0/0/0 (no
/// tiles outside of the main tile pyramid). Used for requesting data;
/// represents data tiles that exist out there. z is never larger than the
/// source's maxzoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, mln_wire::Wire)]
pub struct CanonicalTileID {
    pub z: u8,
    pub x: u32,
    pub y: u32,
}

impl CanonicalTileID {
    #[must_use]
    pub fn new(z: u8, x: u32, y: u32) -> Self {
        debug_assert!(z <= 32);
        debug_assert!(u64::from(x) < (1u64 << z));
        debug_assert!(u64::from(y) < (1u64 << z));
        CanonicalTileID { z, x, y }
    }

    #[must_use]
    pub fn is_child_of(&self, parent: &CanonicalTileID) -> bool {
        // We're first testing for z == 0, to avoid a 32 bit shift, which is
        // undefined.
        parent.z == 0
            || (parent.z < self.z
                && parent.x == self.x.wrapping_shr(u32::from(self.z - parent.z))
                && parent.y == self.y.wrapping_shr(u32::from(self.z - parent.z)))
    }

    /// The tile at zoom level `target_z` that holds this one, or the
    /// top-left of the tiles this one holds there.
    #[must_use]
    pub fn scaled_to(&self, target_z: u8) -> CanonicalTileID {
        if target_z <= self.z {
            // parent or same
            let shift = (i32::from(self.z) - i32::from(target_z)) as u32;
            CanonicalTileID::new(target_z, self.x.wrapping_shr(shift), self.y.wrapping_shr(shift))
        } else {
            // child
            let shift = (i32::from(target_z) - i32::from(self.z)) as u32;
            CanonicalTileID::new(target_z, self.x.wrapping_shl(shift), self.y.wrapping_shl(shift))
        }
    }

    #[must_use]
    pub fn children(&self) -> [CanonicalTileID; 4] {
        let child_z = self.z.wrapping_add(1);
        let child_x = self.x.wrapping_mul(2);
        let child_y = self.y.wrapping_mul(2);
        [
            CanonicalTileID::new(child_z, child_x, child_y),
            CanonicalTileID::new(child_z, child_x, child_y.wrapping_add(1)),
            CanonicalTileID::new(child_z, child_x.wrapping_add(1), child_y),
            CanonicalTileID::new(child_z, child_x.wrapping_add(1), child_y.wrapping_add(1)),
        ]
    }
}

/// `operator<<` and `util::toString`: `z/x/y`.
impl fmt::Display for CanonicalTileID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.z, self.x, self.y)
    }
}

#[cfg(test)]
#[path = "tile_id_test.rs"]
mod tile_id_test;
