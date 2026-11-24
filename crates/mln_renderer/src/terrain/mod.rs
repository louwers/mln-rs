//! 3D terrain's building blocks: the surface's tiles over a raster-dem
//! source and the elevation ranges that cull them ([`cover`]).

#![expect(dead_code, reason = "terrain (#702) wires these into the frame")]

mod cover;

pub(crate) use cover::ElevationRanges;
#[cfg(test)]
pub(crate) use cover::terrain_tile_size;
