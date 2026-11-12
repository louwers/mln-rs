// Ports include/mln/tile/tile_id.hpp, src/mln/tile/tile_id_io.cpp and
// src/mln/tile/tile_id_hash.cpp.
//
// Integer arithmetic follows the C++ types: `uint8_t`/`uint32_t` arithmetic
// wraps, shifts are `wrapping_sh*` (x86 masks the shift count, which is what
// the C++ code compiles to where it would be UB), and the narrowing casts in
// the UnwrappedTileID constructor are `as` casts like `static_cast`.
//
// Ordering: the derived `Ord` compares fields in declaration order, which is
// the `std::tie` order of the C++ `operator<`.
//
// Hashing: `std::hash` specializations combine the fields with
// `util::hash_combine`; the port derives `Hash` (field order differs from the
// C++ combine order, but no ported code depends on hash values).

use std::fmt;

use mln_util::constants;
use serde::{Deserialize, Serialize};

/// Has integer z/x/y coordinates. All tiles must be derived from 0/0/0 (no
/// tiles outside of the main tile pyramid). Used for requesting data;
/// represents data tiles that exist out there. z is never larger than the
/// source's maxzoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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

/// Has integer z/x/y coordinates. `overscaled_z` describes the zoom level
/// this tile is intended to represent, e.g. when parsing data. z is never
/// larger than the source's maxzoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OverscaledTileID {
    pub overscaled_z: u8,
    pub wrap: i16,
    pub canonical: CanonicalTileID,
}

impl OverscaledTileID {
    /// `OverscaledTileID(uint8_t overscaledZ, int16_t wrap, CanonicalTileID)`.
    #[must_use]
    pub fn new(overscaled_z: u8, wrap: i16, canonical: CanonicalTileID) -> Self {
        debug_assert!(overscaled_z >= canonical.z);
        OverscaledTileID { overscaled_z, wrap, canonical }
    }

    /// `OverscaledTileID(uint8_t overscaledZ, int16_t wrap, uint8_t z,
    /// uint32_t x, uint32_t y)`.
    #[must_use]
    pub fn with_zxy(overscaled_z: u8, wrap: i16, z: u8, x: u32, y: u32) -> Self {
        Self::new(overscaled_z, wrap, CanonicalTileID::new(z, x, y))
    }

    /// `OverscaledTileID(uint8_t z, uint32_t x, uint32_t y)`.
    #[must_use]
    pub fn from_zxy(z: u8, x: u32, y: u32) -> Self {
        OverscaledTileID { overscaled_z: z, wrap: 0, canonical: CanonicalTileID::new(z, x, y) }
    }

    #[must_use]
    pub fn overscale_factor(&self) -> u32 {
        1u32.wrapping_shl((i32::from(self.overscaled_z) - i32::from(self.canonical.z)) as u32)
    }

    #[must_use]
    pub fn is_child_of(&self, rhs: &OverscaledTileID) -> bool {
        self.wrap == rhs.wrap
            && self.overscaled_z > rhs.overscaled_z
            && (self.canonical == rhs.canonical || self.canonical.is_child_of(&rhs.canonical))
    }

    #[must_use]
    pub fn scaled_to(&self, z: u8) -> OverscaledTileID {
        OverscaledTileID::new(
            z,
            self.wrap,
            if z >= self.canonical.z { self.canonical } else { self.canonical.scaled_to(z) },
        )
    }

    #[must_use]
    pub fn to_unwrapped(&self) -> UnwrappedTileID {
        UnwrappedTileID::with_wrap(self.wrap, self.canonical)
    }

    #[must_use]
    pub fn unwrap_to(&self, new_wrap: i16) -> OverscaledTileID {
        OverscaledTileID::new(self.overscaled_z, new_wrap, self.canonical)
    }
}

/// `explicit OverscaledTileID(const CanonicalTileID&)`.
impl From<CanonicalTileID> for OverscaledTileID {
    fn from(canonical: CanonicalTileID) -> Self {
        OverscaledTileID::new(canonical.z, 0, canonical)
    }
}

/// `operator<<` and `util::toString`: `z/x/y=>overscaledZ`.
impl fmt::Display for OverscaledTileID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}=>{}", self.canonical, self.overscaled_z)
    }
}

/// Has integer z/x/y coordinates. `wrap` describes tiles that are left/right
/// of the main tile pyramid, e.g. when wrapping the world. Used for describing
/// what position tiles are getting rendered at (= calc the matrix). z is
/// never larger than the source's maxzoom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnwrappedTileID {
    pub wrap: i16,
    pub canonical: CanonicalTileID,
}

impl UnwrappedTileID {
    /// `UnwrappedTileID(uint8_t z, int64_t x, int64_t y)`.
    #[must_use]
    pub fn new(z: u8, x: i64, y: i64) -> Self {
        let dim = 1i64.wrapping_shl(u32::from(z));
        let wrap = ((if x < 0 { x - dim + 1 } else { x }) / dim) as i16;
        let canonical_x = (x - i64::from(wrap) * dim) as u32;
        let canonical_y = if y < 0 {
            0
        } else {
            let max = (1u64.wrapping_shl(u32::from(z)) as u32).wrapping_sub(1);
            let y = y as u32;
            if max < y { max } else { y }
        };
        UnwrappedTileID { wrap, canonical: CanonicalTileID::new(z, canonical_x, canonical_y) }
    }

    /// `UnwrappedTileID(int16_t wrap, CanonicalTileID)`.
    #[must_use]
    pub const fn with_wrap(wrap: i16, canonical: CanonicalTileID) -> Self {
        UnwrappedTileID { wrap, canonical }
    }

    #[must_use]
    pub fn is_child_of(&self, parent: &UnwrappedTileID) -> bool {
        self.wrap == parent.wrap && self.canonical.is_child_of(&parent.canonical)
    }

    #[must_use]
    pub fn children(&self) -> [UnwrappedTileID; 4] {
        let child_z = self.canonical.z.wrapping_add(1);
        let child_x = self.canonical.x.wrapping_mul(2);
        let child_y = self.canonical.y.wrapping_mul(2);
        [
            UnwrappedTileID::with_wrap(self.wrap, CanonicalTileID::new(child_z, child_x, child_y)),
            UnwrappedTileID::with_wrap(self.wrap, CanonicalTileID::new(child_z, child_x, child_y.wrapping_add(1))),
            UnwrappedTileID::with_wrap(self.wrap, CanonicalTileID::new(child_z, child_x.wrapping_add(1), child_y)),
            UnwrappedTileID::with_wrap(
                self.wrap,
                CanonicalTileID::new(child_z, child_x.wrapping_add(1), child_y.wrapping_add(1)),
            ),
        ]
    }

    #[must_use]
    pub fn overscale_to(&self, overscaled_z: u8) -> OverscaledTileID {
        debug_assert!(overscaled_z >= self.canonical.z);
        OverscaledTileID::new(overscaled_z, self.wrap, self.canonical)
    }

    #[must_use]
    pub fn pixels_to_tile_units(&self, pixel_value: f32, zoom: f32) -> f32 {
        pixel_value
            * (constants::EXTENT as f32
                / (constants::TILE_SIZE as f32 * 2.0f32.powf(zoom - f32::from(self.canonical.z))))
    }

    #[must_use]
    pub fn unwrap_to(&self, new_wrap: i16) -> UnwrappedTileID {
        UnwrappedTileID::with_wrap(new_wrap, self.canonical)
    }
}

/// `operator<<` and `util::toString`: `z/x/y+wrap` (or `z/x/y-wrap`).
impl fmt::Display for UnwrappedTileID {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}{}", self.canonical, if self.wrap >= 0 { "+" } else { "" }, self.wrap)
    }
}

#[cfg(test)]
#[path = "tile_id_test.rs"]
mod tile_id_test;
