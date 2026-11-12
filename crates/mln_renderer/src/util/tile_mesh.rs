// Ports include/mln/util/tile_mesh.hpp and src/mln/util/tile_mesh.cpp.
//
// C++ throws `std::invalid_argument` for a granularity past the 16-bit index
// space, which the subdivision granularity settings never ask for; the port
// panics with the same message, as a broken invariant.

use mln_util::constants::EXTENT;

use crate::util::subdivision::{NORTH_POLE_Y, SOUTH_POLE_Y};

const EXTENT_STENCIL_BORDER: i32 = EXTENT / 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileMeshOptions {
    /// Quads per axis; 1 is the plain tile quad.
    pub granularity: u32,
    /// Adds a ring of quads `EXTENT / 128` units outside the tile, for
    /// stencil masks.
    pub generate_borders: bool,
    /// Replaces the north border with geometry reaching the north pole
    /// vertex.
    pub extend_to_north_pole: bool,
    pub extend_to_south_pole: bool,
}

impl Default for TileMeshOptions {
    fn default() -> Self {
        TileMeshOptions {
            granularity: 1,
            generate_borders: false,
            extend_to_north_pole: false,
            extend_to_south_pole: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TileMesh {
    /// Flattened `x, y` pairs in tile units.
    pub vertices: Vec<i16>,
    pub indices: Vec<u16>,
}

/// A grid mesh covering one tile, optionally with borders and pole caps; port
/// of GL JS `util/create_tile_mesh.ts`. Triangles are counter-clockwise in
/// tile space.
#[must_use]
pub fn create_tile_mesh(options: &TileMeshOptions) -> TileMesh {
    // Capped first so the product cannot wrap; every granularity past the
    // vertex limit fails the check anyway.
    const MAX_VERTICES: u64 = 1 << 16;
    let requested_granularity = u64::from(options.granularity.max(1)).min(MAX_VERTICES);
    let extra_quads_x: u64 = if options.generate_borders { 2 } else { 0 };
    let extra_quads_y: u64 = u64::from(options.extend_to_north_pole || options.generate_borders)
        + u64::from(options.extend_to_south_pole || options.generate_borders);
    if (requested_granularity + extra_quads_x + 1) * (requested_granularity + extra_quads_y + 1) > MAX_VERTICES {
        panic!("Tile mesh granularity is too large for 16 bit indices.");
    }
    let granularity = requested_granularity as i32;
    let quads_per_axis_x = granularity + extra_quads_x as i32;
    let quads_per_axis_y = granularity + extra_quads_y as i32;
    let vertices_per_axis_x = quads_per_axis_x + 1;
    let vertices_per_axis_y = quads_per_axis_y + 1;
    let offset_x = if options.generate_borders { -1 } else { 0 };
    let offset_y = if options.generate_borders || options.extend_to_north_pole { -1 } else { 0 };
    let end_x = granularity + i32::from(options.generate_borders);
    let end_y = granularity + i32::from(options.generate_borders || options.extend_to_south_pole);

    let mut mesh = TileMesh::default();
    mesh.vertices.reserve(vertices_per_axis_x as usize * vertices_per_axis_y as usize * 2);
    for y in offset_y..=end_y {
        for x in offset_x..=end_x {
            let mut vx = x * EXTENT / granularity;
            if x == -1 {
                vx = -EXTENT_STENCIL_BORDER;
            }
            if x == granularity + 1 {
                vx = EXTENT + EXTENT_STENCIL_BORDER;
            }
            let mut vy = y * EXTENT / granularity;
            if y == -1 {
                vy = if options.extend_to_north_pole { i32::from(NORTH_POLE_Y) } else { -EXTENT_STENCIL_BORDER };
            }
            if y == granularity + 1 {
                vy =
                    if options.extend_to_south_pole { i32::from(SOUTH_POLE_Y) } else { EXTENT + EXTENT_STENCIL_BORDER };
            }
            mesh.vertices.push(vx as i16);
            mesh.vertices.push(vy as i16);
        }
    }

    mesh.indices.reserve(quads_per_axis_x as usize * quads_per_axis_y as usize * 6);
    for y in 0..quads_per_axis_y {
        for x in 0..quads_per_axis_x {
            let v0 = (x + y * vertices_per_axis_x) as u16;
            let v1 = ((x + 1) + y * vertices_per_axis_x) as u16;
            let v2 = (x + (y + 1) * vertices_per_axis_x) as u16;
            let v3 = ((x + 1) + (y + 1) * vertices_per_axis_x) as u16;
            mesh.indices.extend_from_slice(&[v0, v2, v1, v1, v2, v3]);
        }
    }
    mesh
}
