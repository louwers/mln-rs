// Ported gtests of test/tile/*.test.cpp that need the renderer's tiles or
// mln_map's Transform (above mln_renderer), one module per C++ file. The
// other test/tile files are ported elsewhere: geojson_tile (geojson_tile_test),
// vector_tile (vector_tile_test, mln_tile), geometry_tile_data (mln_tile),
// tile_cache (source_test), tile_id (mln_tile_id), tile_lod (render_test,
// which has the HeadlessFrontend).

mod custom_geometry_tile;
mod custom_vector_tile;
mod raster_dem_tile;
mod raster_tile;
mod tile_coordinate;

use std::sync::Arc;

use mln_map_types::MapMode;
use mln_tile_id::OverscaledTileID;
use mln_util::pool::{Pool, TaskGroup};

use crate::tests::OneTile;
use crate::tile::tile::TileKind;
use crate::tile::tile_workers::TileWorkers;

/// The fixture shared by `RasterTileTest` and `RasterDEMTileTest`
/// (`test/tile/raster_tile.test.cpp`, `raster_dem_tile.test.cpp`): the
/// workers' pool, which the tiles' stores free on, and a pool without
/// threads, which the test runs itself.
pub(crate) struct RasterTileTest {
    pub(crate) thread_pool: TaskGroup,
    pub(crate) local_pool: Pool,
}

impl RasterTileTest {
    #[must_use]
    pub(crate) fn new() -> Self {
        RasterTileTest { thread_pool: TaskGroup::new(&crate::tests::pool()), local_pool: Pool::local() }
    }

    /// Tile `id` of `kind`, whose jobs run on `workers`.
    pub(crate) fn tiles(&self, kind: TileKind, id: OverscaledTileID, workers: Box<dyn TileWorkers>) -> OneTile {
        let store = crate::tests::tile_store(&self.thread_pool, workers);
        OneTile::new(store, kind, id, MapMode::Static)
    }

    /// Tile 0/0/0 of `kind`, whose jobs run on the thread pool.
    pub(crate) fn pool_tiles(&self, kind: TileKind) -> OneTile {
        let store = crate::tests::pool_store(&self.thread_pool);
        OneTile::new(store, kind, OverscaledTileID::from_zxy(0, 0, 0), MapMode::Static)
    }

    /// Tiles 1/0/0 and 1/1/0 of `kind`, whose jobs run in `count` worker
    /// instances on threads, reached only through the worker codec.
    pub(crate) fn remote_tiles(&self, kind: TileKind, count: usize) -> OneTile {
        let workers = Box::new(remote_workers(count));
        let mut tiles = self.tiles(kind, OverscaledTileID::from_zxy(1, 0, 0), workers);
        tiles.add(OverscaledTileID::from_zxy(1, 1, 0));
        tiles
    }
}

impl Default for RasterTileTest {
    fn default() -> Self {
        Self::new()
    }
}

/// Port-only: workers in `count` worker instances on threads, reached only
/// through the worker codec.
#[must_use]
pub(crate) fn remote_workers(count: usize) -> crate::tile::worker_codec::RemoteWorkers {
    use crate::tile::worker_codec::{RemoteWorkers, ThreadWorkerInstances};
    let registry = crate::Registry::with_builtins();
    RemoteWorkers::new(Arc::clone(&registry), move |replies| {
        Box::new(ThreadWorkerInstances::new(count, registry, &replies))
    })
}

/// Port-only: an opaque PNG whose pixels differ from their neighbours.
#[must_use]
pub(crate) fn png(width: u32, height: u32) -> mln_storage::Data {
    let mut image = mln_style::bitmap::PremultipliedImage::new(mln_style::size::Size::new(width, height));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        pixel.copy_from_slice(&[(i % 251) as u8, (i / 7 % 253) as u8, (i * 3 % 255) as u8, 255]);
    }
    Arc::new(mln_image::encode_png(&image))
}

/// Port-only: a 16 by 16 DEM image whose pixels differ from their
/// neighbours and from those of other `seed`s.
#[must_use]
pub(crate) fn dem_png(seed: u32) -> mln_storage::Data {
    let mut image = mln_style::bitmap::PremultipliedImage::new(mln_style::size::Size::new(16, 16));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let i = i as u32;
        *pixel = [((seed * 37 + i) % 256) as u8, (i * 7 % 256) as u8, ((i * 3 + seed) % 256) as u8, 255];
    }
    Arc::new(mln_image::encode_png(&image))
}

impl Drop for RasterTileTest {
    fn drop(&mut self) {
        self.thread_pool.wait();
    }
}
