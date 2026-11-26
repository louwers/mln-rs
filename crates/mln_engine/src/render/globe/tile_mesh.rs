use mln_style::constants::EXTENT;

use crate::render::globe::subdivision::{NORTH_POLE_Y, SOUTH_POLE_Y};

const EXTENT_STENCIL_BORDER: i32 = EXTENT / 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TileMeshOptions {
    /// Quads per axis; 1 is the plain tile quad. Capped so that every index
    /// fits in 16 bits.
    pub(crate) granularity: u32,
    /// Adds a ring of quads `EXTENT / 128` units outside the tile, for
    /// stencil masks.
    pub(crate) generate_borders: bool,
    /// Replaces the north border with geometry reaching the north pole
    /// vertex.
    pub(crate) extend_to_north_pole: bool,
    pub(crate) extend_to_south_pole: bool,
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
pub(crate) struct TileMesh {
    /// The vertices' `x, y` in tile units.
    pub(crate) vertices: Vec<[i16; 2]>,
    pub(crate) triangles: Vec<[u16; 3]>,
}

/// A grid mesh covering one tile, optionally with borders and pole caps, as
/// GL JS's `util/create_tile_mesh.ts` makes it. Triangles are
/// counter-clockwise in tile space.
#[must_use]
pub(crate) fn create_tile_mesh(options: TileMeshOptions) -> TileMesh {
    let extra_quads_x: i32 = if options.generate_borders { 2 } else { 0 };
    let extra_quads_y = i32::from(options.extend_to_north_pole || options.generate_borders)
        + i32::from(options.extend_to_south_pole || options.generate_borders);
    // At most 256 vertices per axis, so every index fits in 16 bits.
    let max_granularity = 255 - extra_quads_x.max(extra_quads_y);
    let granularity = options.granularity.clamp(1, max_granularity as u32) as i32;
    let quads_per_axis_x = granularity + extra_quads_x;
    let quads_per_axis_y = granularity + extra_quads_y;
    let vertices_per_axis_x = quads_per_axis_x + 1;
    let vertices_per_axis_y = quads_per_axis_y + 1;
    let offset_x = if options.generate_borders { -1 } else { 0 };
    let offset_y = if options.generate_borders || options.extend_to_north_pole { -1 } else { 0 };
    let end_x = granularity + i32::from(options.generate_borders);
    let end_y = granularity + i32::from(options.generate_borders || options.extend_to_south_pole);

    let mut mesh = TileMesh::default();
    mesh.vertices.reserve(vertices_per_axis_x as usize * vertices_per_axis_y as usize);
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
            mesh.vertices.push([vx as i16, vy as i16]);
        }
    }

    mesh.triangles.reserve(quads_per_axis_x as usize * quads_per_axis_y as usize * 2);
    for y in 0..quads_per_axis_y {
        for x in 0..quads_per_axis_x {
            let v0 = (x + y * vertices_per_axis_x) as u16;
            let v1 = ((x + 1) + y * vertices_per_axis_x) as u16;
            let v2 = (x + (y + 1) * vertices_per_axis_x) as u16;
            let v3 = ((x + 1) + (y + 1) * vertices_per_axis_x) as u16;
            mesh.triangles.extend([[v0, v2, v1], [v1, v2, v3]]);
        }
    }
    mesh
}
