//! 3D terrain's building blocks: the surface's tiles over a raster-dem
//! source and the elevation ranges that cull them ([`cover`]), the ground's
//! height on the CPU by the height rule of `mln_util::dem` ([`elevation`]),
//! the mesh each surface tile draws ([`mesh`]), and the stacks of layers
//! that drape over the surface ([`stacks`]).

#![expect(dead_code, reason = "terrain (#702) wires these into the frame")]

mod cover;
mod elevation;
mod mesh;
mod stacks;

pub(crate) use cover::ElevationRanges;
#[cfg(test)]
pub(crate) use cover::terrain_tile_size;
