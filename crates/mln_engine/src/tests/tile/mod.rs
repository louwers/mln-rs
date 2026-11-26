// Ported gtests of test/tile/*.test.cpp that need the renderer's tiles or
// the camera's transform, one module per C++ file. The
// other test/tile files are ported elsewhere: geojson_tile (geojson_tile_test),
// vector_tile (vector_tile_test, `tile`), geometry_tile_data (`tile`),
// tile_cache (source_test), tile_id (`tile::tile_id`), tile_lod (render_test,
// which has the HeadlessFrontend).

mod custom_geometry_tile;
mod custom_vector_tile;
mod raster_dem_tile;
mod raster_tile;
mod tile_coordinate;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mln_style::layer::SourceKind;
use mln_style::sources::CustomSourceHandle;
use mln_style::tile_id::CanonicalTileID;
use parking_lot::Mutex;

use crate::exec::pool::{Pool, TaskGroup};
use crate::map::MapMode;
use crate::tests::OneTile;
use crate::tile::OverscaledTileID;
use crate::worker::tile_workers::TileWorkers;

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
    pub(crate) fn tiles(&self, kind: SourceKind, id: OverscaledTileID, workers: Box<dyn TileWorkers>) -> OneTile {
        let store = crate::tests::tile_store(&self.thread_pool, workers);
        OneTile::new(store, kind, id, MapMode::Static)
    }

    /// Tile 0/0/0 of `kind`, whose jobs run on the thread pool.
    pub(crate) fn pool_tiles(&self, kind: SourceKind) -> OneTile {
        let store = crate::tests::pool_store(&self.thread_pool);
        OneTile::new(store, kind, OverscaledTileID::from_zxy(0, 0, 0), MapMode::Static)
    }

    /// Tiles 1/0/0 and 1/1/0 of `kind`, whose jobs run in `count` worker
    /// instances on threads, reached only through the worker codec.
    pub(crate) fn remote_tiles(&self, kind: SourceKind, count: usize) -> OneTile {
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

/// A host's tile function, as the custom source tests share it.
pub(crate) type TileFunction = Arc<dyn Fn(&CanonicalTileID) + Send + Sync>;

/// Port-only: the host of a custom source, whose tile functions keep the
/// handle that the source's provider calls them with, which the test
/// answers through.
pub(crate) struct Host<D> {
    handle: Arc<Mutex<Option<CustomSourceHandle<D>>>>,
}

impl<D: 'static> Host<D> {
    pub(crate) fn new() -> Self {
        Host { handle: Arc::default() }
    }

    /// A tile function of the host that keeps the handle, then calls
    /// `function`, if there is one.
    pub(crate) fn function(&self, function: Option<TileFunction>) -> mln_style::sources::custom::TileFunction<D> {
        let kept = Arc::clone(&self.handle);
        Arc::new(move |tile_id, handle| {
            kept.lock().get_or_insert_with(|| handle.clone());
            if let Some(function) = &function {
                function(tile_id);
            }
        })
    }

    /// The handle of the source's provider.
    ///
    /// # Panics
    ///
    /// If the provider hasn't called a tile function yet.
    pub(crate) fn handle(&self) -> CustomSourceHandle<D> {
        self.handle.lock().clone().expect("the provider called a tile function")
    }
}

/// A tile function that counts its calls.
pub(crate) fn counter() -> (Arc<AtomicUsize>, TileFunction) {
    let count = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&count);
    (
        count,
        Arc::new(move |_: &CanonicalTileID| {
            counted.fetch_add(1, Ordering::SeqCst);
        }),
    )
}

/// A tile function that records the tiles it's called with.
pub(crate) fn recorder() -> (Arc<Mutex<Vec<CanonicalTileID>>>, TileFunction) {
    let ids = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&ids);
    (ids, Arc::new(move |id: &CanonicalTileID| recorded.lock().push(*id)))
}

/// Tiles `ids` of the custom source `source`, whose jobs run on this
/// thread, and whose store, with the provider of the source's tiles, runs
/// its calls on the test's local pool.
pub(crate) fn custom_tiles(test: &RasterTileTest, source: &mln_style::Source, ids: &[OverscaledTileID]) -> OneTile {
    let pool = TaskGroup::new(&test.local_pool);
    let store = crate::tests::tile_store(&pool, Box::new(crate::worker::tile_workers::InlineWorkers));
    OneTile::with_origin(store, source, None, ids, MapMode::Static)
}

/// Port-only: workers in `count` worker instances on threads, reached only
/// through the worker codec.
#[must_use]
pub(crate) fn remote_workers(count: usize) -> crate::worker::worker_codec::RemoteWorkers {
    use crate::worker::worker_codec::{RemoteWorkers, ThreadWorkerInstances};
    let registry = crate::Registry::with_builtins();
    RemoteWorkers::new(Arc::clone(&registry), move |replies| {
        Box::new(ThreadWorkerInstances::new(count, registry, &replies))
    })
}

/// Port-only: an opaque PNG whose pixels differ from their neighbours.
#[must_use]
pub(crate) fn png(width: u32, height: u32) -> crate::storage::Data {
    let mut image = mln_style::bitmap::PremultipliedImage::new(mln_style::size::Size::new(width, height));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        pixel.copy_from_slice(&[(i % 251) as u8, (i / 7 % 253) as u8, (i * 3 % 255) as u8, 255]);
    }
    Arc::new(crate::image::encode_png(&image))
}

/// Port-only: a 16 by 16 DEM image whose pixels differ from their
/// neighbours and from those of other `seed`s.
#[must_use]
pub(crate) fn dem_png(seed: u32) -> crate::storage::Data {
    let mut image = mln_style::bitmap::PremultipliedImage::new(mln_style::size::Size::new(16, 16));
    for (i, pixel) in image.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let i = i as u32;
        *pixel = [((seed * 37 + i) % 256) as u8, (i * 7 % 256) as u8, ((i * 3 + seed) % 256) as u8, 255];
    }
    Arc::new(crate::image::encode_png(&image))
}

impl Drop for RasterTileTest {
    fn drop(&mut self) {
        self.thread_pool.wait();
    }
}
