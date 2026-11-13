// Ports src/mln/renderer/buckets/hillshade_bucket.hpp and hillshade_bucket.cpp
//
// As RasterBucket (see raster_bucket.rs), the bucket is shared
// (`Arc<dyn Bucket>`) between its tile, which changes the mask and
// backfills the DEM borders on the render thread, and the render layers.
// So the masked geometry and the DEM are behind locks.
//
// C++ also keeps the prepare render target in the bucket (`renderTarget`,
// `renderTargetPrepared`); render targets are render-thread objects
// (`Rc`), so the render layer keeps them per bucket id, and the bucket
// counts the C++ `renderTargetPrepared = false` resets
// (`render_target_generation`) so the layer can tell when to prepare again.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use geometry::Point;
use mln_gfx::UploadPass;
use mln_gfx::shaders::segment::SegmentBase;
use mln_tile_id::CanonicalTileID;
use mln_util::constants;
use mln_util::image::PremultipliedImage;
use mln_util::tileset::RasterEncoding;
use mln_util::time::Instant;
use parking_lot::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use serde::{Deserialize, Serialize};

use crate::bucket::{Bucket, BucketBase};
use crate::geometry::dem_data::DEMData;
use crate::tile_mask::TileMask;

/// `HillshadeLayoutVertex = gfx::Vertex<TypeList<attributes::pos, attributes::texture_pos>>`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct HillshadeLayoutVertex {
    pub a1: [i16; 2],
    pub a2: [u16; 2],
}

/// The masked geometry of a hillshade bucket (C++ `mask`, `vertices`,
/// `indices`, `segments`).
pub struct HillshadeBucketGeometry {
    pub mask: TileMask,
    pub vertices: Vec<HillshadeLayoutVertex>,
    /// `gfx::IndexVector<gfx::Triangles>`.
    pub indices: Vec<u16>,
    pub segments: Vec<SegmentBase>,
    /// `vertices.getLastModified()`.
    pub last_modified: Instant,
}

/// `TileMask{{0, 0, 0}}`.
fn full_mask() -> TileMask {
    let mut mask = TileMask::new();
    mask.insert(CanonicalTileID::new(0, 0, 0));
    mask
}

/// `HillshadeBucket`.
///
/// Port-only: serde writes the DEM, border included, which is what a worker
/// sets (`tile/worker_codec`) and the tile backfills from its neighbours; the
/// geometry is the tile's (`setMask`) and the render state this side's.
#[derive(Serialize, Deserialize)]
pub struct HillshadeBucket {
    pub base: BucketBase,
    #[serde(skip, default = "unmasked")]
    geometry: Mutex<HillshadeBucketGeometry>,
    demdata: RwLock<DEMData>,
    #[serde(skip)]
    prepared: AtomicBool,
    /// Incremented where C++ sets `renderTargetPrepared = false`.
    #[serde(skip)]
    render_target_generation: AtomicU64,
}

fn unmasked() -> Mutex<HillshadeBucketGeometry> {
    Mutex::new(HillshadeBucketGeometry {
        mask: full_mask(),
        vertices: Vec::new(),
        indices: Vec::new(),
        segments: Vec::new(),
        last_modified: Instant::now(),
    })
}

impl HillshadeBucket {
    /// `HillshadeBucket(PremultipliedImage&&, Tileset::RasterEncoding)`;
    /// the `DEMData` constructor's exception is the `Err`.
    pub fn new(image: &PremultipliedImage, encoding: RasterEncoding) -> Result<Self, String> {
        Ok(Self::with_dem_data(DEMData::new(image, encoding)?))
    }

    /// `HillshadeBucket(DEMData&&)`.
    #[must_use]
    pub fn with_dem_data(demdata: DEMData) -> Self {
        HillshadeBucket {
            base: BucketBase::default(),
            geometry: unmasked(),
            demdata: RwLock::new(demdata),
            prepared: AtomicBool::new(false),
            render_target_generation: AtomicU64::new(0),
        }
    }

    /// `layoutVertex(Point<int16_t> p, Point<uint16_t> t)`.
    #[must_use]
    pub fn layout_vertex(p: Point<i16>, t: Point<u16>) -> HillshadeLayoutVertex {
        HillshadeLayoutVertex { a1: [p.x, p.y], a2: [t.x, t.y] }
    }

    /// The masked geometry (C++ reads the members directly).
    pub fn geometry(&self) -> MutexGuard<'_, HillshadeBucketGeometry> {
        self.geometry.lock()
    }

    /// `getDEMData() const`.
    pub fn get_dem_data(&self) -> RwLockReadGuard<'_, DEMData> {
        self.demdata.read()
    }

    /// `getDEMData()`.
    pub fn get_dem_data_mut(&self) -> RwLockWriteGuard<'_, DEMData> {
        self.demdata.write()
    }

    pub fn is_prepared(&self) -> bool {
        self.prepared.load(Ordering::SeqCst)
    }

    pub fn set_prepared(&self, prepared_state: bool) {
        self.prepared.store(prepared_state, Ordering::SeqCst);
    }

    /// `renderTargetPrepared = false`.
    pub fn reset_render_target_prepared(&self) {
        self.render_target_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// The number of `renderTargetPrepared = false` resets so far.
    pub fn render_target_generation(&self) -> u64 {
        self.render_target_generation.load(Ordering::SeqCst)
    }

    /// `clear()`.
    pub fn clear(&self) {
        let mut geometry = self.geometry();
        Self::clear_geometry(&mut geometry);
        self.base.uploaded.store(false, Ordering::SeqCst);
    }

    fn clear_geometry(geometry: &mut HillshadeBucketGeometry) {
        geometry.segments.clear();
        geometry.vertices.clear();
        geometry.indices.clear();

        geometry.last_modified = Instant::now();
    }

    /// `setMask(TileMask&&)`.
    pub fn set_mask(&self, mask: TileMask) {
        const VERTEX_LENGTH: usize = 4;

        let mut geometry = self.geometry();
        if geometry.mask == mask {
            return;
        }

        geometry.mask = mask;
        Self::clear_geometry(&mut geometry);
        self.base.uploaded.store(false, Ordering::SeqCst);

        if geometry.mask == full_mask() {
            // We want to render the full tile, and keeping the
            // segments/vertices/indices empty means using the global shared
            // buffers for covering the entire tile.
            return;
        }

        // Create a new segment so that we will upload (empty) buffers even
        // when there is nothing to draw for this tile.
        geometry.segments.push(SegmentBase::new(0, 0));

        // Create the vertex buffer for the specified tile mask.
        let ids: Vec<CanonicalTileID> = geometry.mask.iter().copied().collect();
        for id in ids {
            // Create a quad for every masked tile.
            let vertex_extent: i32 = constants::EXTENT >> id.z;

            // C++ static_cast<int16_t> of `id.x * vertexExtent` (int32).
            let tl_vertex = Point::new(
                (id.x as i32).wrapping_mul(vertex_extent) as i16,
                (id.y as i32).wrapping_mul(vertex_extent) as i16,
            );
            let br_vertex = Point::new(
                (i32::from(tl_vertex.x) + vertex_extent) as i16,
                (i32::from(tl_vertex.y) + vertex_extent) as i16,
            );

            if geometry.segments.last().expect("segment").vertex_length + VERTEX_LENGTH > usize::from(u16::MAX) {
                // Move to a new segments because the old one can't hold the
                // geometry.
                let (v, i) = (geometry.vertices.len(), geometry.indices.len());
                geometry.segments.push(SegmentBase::new(v, i));
            }

            geometry.vertices.push(Self::layout_vertex(
                Point::new(tl_vertex.x, tl_vertex.y),
                Point::new(tl_vertex.x as u16, tl_vertex.y as u16),
            ));
            geometry.vertices.push(Self::layout_vertex(
                Point::new(br_vertex.x, tl_vertex.y),
                Point::new(br_vertex.x as u16, tl_vertex.y as u16),
            ));
            geometry.vertices.push(Self::layout_vertex(
                Point::new(tl_vertex.x, br_vertex.y),
                Point::new(tl_vertex.x as u16, br_vertex.y as u16),
            ));
            geometry.vertices.push(Self::layout_vertex(
                Point::new(br_vertex.x, br_vertex.y),
                Point::new(br_vertex.x as u16, br_vertex.y as u16),
            ));

            let segment = geometry.segments.last().expect("segment");
            debug_assert!(u16::try_from(segment.vertex_length).is_ok());
            let offset = segment.vertex_length as u16;

            // 0, 1, 2
            // 1, 2, 3
            geometry.indices.extend_from_slice(&[offset, offset + 1, offset + 2]);
            geometry.indices.extend_from_slice(&[offset + 1, offset + 2, offset + 3]);

            let segment = geometry.segments.last_mut().expect("segment");
            segment.vertex_length += VERTEX_LENGTH;
            segment.index_length += 6;
        }

        geometry.last_modified = Instant::now();
    }
}

impl Bucket for HillshadeBucket {
    fn base(&self) -> &BucketBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut BucketBase {
        &mut self.base
    }

    fn upload(&self, _upload_pass: &dyn UploadPass) {
        if !self.has_data() {
            return;
        }
        self.base.uploaded.store(true, Ordering::SeqCst);
    }

    fn has_data(&self) -> bool {
        self.get_dem_data().get_image().valid()
    }
}
