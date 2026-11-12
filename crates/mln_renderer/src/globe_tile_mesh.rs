// Ports src/mln/renderer/globe_tile_mesh.hpp.
//
// The C++ templates take the layout function's type; here it is a function
// pointer, `fn(Point<i16>, Point<u16>) -> V`, as `decltype(&layoutVertex)`
// is in C++.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use geometry::Point;
use mln_gfx::index_vector::{IndexVectorBase, IndexVectorBasePtr};
use mln_gfx::shaders::segment::SegmentBase;
use mln_gfx::vertex_vector::VertexVector;
use mln_tile_id::CanonicalTileID;
use mln_util::constants::EXTENT;
use mln_util::subdivision_granularity::{SubdivisionGranularityExpression, SubdivisionGranularitySetting};

use crate::tile_mask::TileMask;
use crate::util::subdivision::{NORTH_POLE_Y, SOUTH_POLE_Y};
use crate::util::tile_mesh::{TileMeshOptions, create_tile_mesh};

/// The pole-capped globe grid for the parts of a tile its mask keeps, as
/// position + texture-position vertices. Each part is gridded like the tile
/// it stands in for, so it meets that tile's neighbours edge to edge; texture
/// positions stay inside the tile so the pole rows sample the tile's edge.
pub struct GlobeTileMesh<V: bytemuck::Pod> {
    pub vertices: Rc<VertexVector<V>>,
    pub indices: IndexVectorBasePtr,
    pub segments: Vec<SegmentBase>,
}

impl<V: bytemuck::Pod> GlobeTileMesh<V> {
    pub fn new(canonical: &CanonicalTileID, mask: &TileMask, layout_vertex: fn(Point<i16>, Point<u16>) -> V) -> Self {
        let mut vertices: Vec<V> = Vec::new();
        let mut indices: Vec<u16> = Vec::new();
        let mut segments: Vec<SegmentBase> = Vec::new();
        for part in mask {
            let mesh = create_tile_mesh(&TileMeshOptions {
                granularity: SubdivisionGranularitySetting::globe()
                    .tile
                    .get_granularity_for_zoom_level(canonical.z.wrapping_add(part.z)),
                generate_borders: false,
                extend_to_north_pole: canonical.y == 0 && part.y == 0,
                extend_to_south_pole: canonical.y == (1u32 << canonical.z) - 1 && part.y == (1u32 << part.z) - 1,
            });
            let vertex_count = mesh.vertices.len() / 2;
            if segments.last().is_none_or(|last| last.vertex_length + vertex_count > usize::from(u16::MAX)) {
                segments.push(SegmentBase::new(vertices.len(), indices.len()));
            }
            let segment = segments.last_mut().expect("segment");
            let offset = segment.vertex_length;
            let extent = EXTENT >> part.z;
            for point in mesh.vertices.as_chunks::<2>().0 {
                let vy = point[1];
                let position = Point::new(
                    (part.x as i32 * extent + (i32::from(point[0]) >> part.z)) as i16,
                    if vy == NORTH_POLE_Y || vy == SOUTH_POLE_Y {
                        vy
                    } else {
                        (part.y as i32 * extent + (i32::from(vy) >> part.z)) as i16
                    },
                );
                // std::clamp<int32_t>(position, 0, EXTENT)
                let clamp = |value: i16| i32::from(value).clamp(0, EXTENT) as u16;
                let texture = Point::new(clamp(position.x), clamp(position.y));
                vertices.push(layout_vertex(position, texture));
            }
            for triangle in mesh.indices.as_chunks::<3>().0 {
                indices.push((offset + usize::from(triangle[0])) as u16);
                indices.push((offset + usize::from(triangle[1])) as u16);
                indices.push((offset + usize::from(triangle[2])) as u16);
            }
            segment.vertex_length += vertex_count;
            segment.index_length += mesh.indices.len();
        }
        GlobeTileMesh {
            vertices: Rc::new(VertexVector::from_vec(vertices)),
            indices: Rc::new(RefCell::new(IndexVectorBase::new(indices))),
            segments,
        }
    }
}

struct Entry<V: bytemuck::Pod> {
    mesh: GlobeTileMesh<V>,
    last_use: u64,
}

/// The grids a layer has built so far, one per zoom, pole row and mask: the
/// mesh depends on nothing else.
pub struct GlobeTileMeshCache<V: bytemuck::Pod> {
    uses: u64,
    meshes: BTreeMap<(u8, bool, bool, TileMask), Entry<V>>,
}

impl<V: bytemuck::Pod> Default for GlobeTileMeshCache<V> {
    fn default() -> Self {
        GlobeTileMeshCache { uses: 0, meshes: BTreeMap::new() }
    }
}

impl<V: bytemuck::Pod> GlobeTileMeshCache<V> {
    const MAX_MESHES: usize = 64;

    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(
        &mut self,
        canonical: &CanonicalTileID,
        mask: &TileMask,
        layout_vertex: fn(Point<i16>, Point<u16>) -> V,
    ) -> &GlobeTileMesh<V> {
        let key = (canonical.z, canonical.y == 0, canonical.y == (1u32 << canonical.z) - 1, mask.clone());
        if !self.meshes.contains_key(&key) {
            // Tiles that are loading go through many masks; the one used
            // longest ago makes room, so the meshes every frame draws stay.
            // The drawables share the meshes' buffers, so they keep theirs.
            if self.meshes.len() >= Self::MAX_MESHES {
                // std::min_element: the first of the least recently used.
                let oldest = self
                    .meshes
                    .iter()
                    .reduce(|oldest, entry| if entry.1.last_use < oldest.1.last_use { entry } else { oldest })
                    .map(|(key, _)| key.clone())
                    .expect("meshes");
                self.meshes.remove(&oldest);
            }
            let mesh = GlobeTileMesh::new(canonical, mask, layout_vertex);
            self.meshes.insert(key.clone(), Entry { mesh, last_use: 0 });
        }
        self.uses += 1;
        let entry = self.meshes.get_mut(&key).expect("mesh");
        entry.last_use = self.uses;
        &entry.mesh
    }

    pub fn clear(&mut self) {
        self.meshes.clear();
    }

    #[must_use]
    pub fn size(&self) -> usize {
        self.meshes.len()
    }
}

/// The same grid as raw `Short2` positions, for drawables built from raw
/// vertex bytes.
#[derive(Debug, Default)]
pub struct RawGlobeTileMesh {
    pub vertices: Vec<u8>,
    pub vertex_count: usize,
    pub indices: Vec<u16>,
    pub segments: Vec<SegmentBase>,
}

/// `rawGlobeTileMesh(canonical, generateBorders, granularity)`; C++ defaults
/// `granularity` to the globe's tile granularity.
#[must_use]
pub fn raw_globe_tile_mesh(
    canonical: &CanonicalTileID,
    generate_borders: bool,
    granularity: &SubdivisionGranularityExpression,
) -> RawGlobeTileMesh {
    let mesh = create_tile_mesh(&TileMeshOptions {
        granularity: granularity.get_granularity_for_zoom_level(canonical.z),
        generate_borders,
        extend_to_north_pole: canonical.y == 0,
        extend_to_south_pole: canonical.y == (1u32 << canonical.z) - 1,
    });
    let vertex_count = mesh.vertices.len() / 2;
    debug_assert!(u16::try_from(vertex_count).is_ok());
    let index_count = mesh.indices.len();
    RawGlobeTileMesh {
        vertices: bytemuck::cast_slice(&mesh.vertices).to_vec(),
        vertex_count,
        indices: mesh.indices,
        segments: vec![SegmentBase::with_lengths(0, 0, vertex_count, index_count)],
    }
}
