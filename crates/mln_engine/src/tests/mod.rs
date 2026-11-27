// Ports test/gl/bucket.test.cpp (circle, fill, line; on the WebGPU headless
// backend) and test/renderer/fill_bucket.test.cpp.
mod bucket_test;
// Cameras that move faster than the network answers.
mod fast_camera_test;
mod fixture_log;
// Ports test/layout/sort_key.test.cpp.
mod gtest_sky;
mod gtest_subdivision;
mod sort_key_test;
// gtests-data: the test/tile gtests that need the renderer's tiles or
// the camera's transform (tests/tile/main.rs lists them).
mod tile;
// gtests-data: the test/text gtests that need mln_engine (GlyphStore,
// SymbolLayout) or mln_style's Formatted (tests/text/main.rs lists them).
mod text;
// gtests-core: test/renderer/image_manager.test.cpp.
mod gtest_image_store;
// gtests-core: test/renderer/dynamic_texture_atlas.test.cpp.
mod gtest_dynamic_texture_atlas;
mod gtest_fill_mesh;
mod gtest_tile_lod;
// gtests-core: test/algorithm/update_renderables.test.cpp.
mod gtest_update_renderables;
// Ports test/util/tile_range.test.cpp (marker `// gtest: Group.Name`).
mod gtest_tile_range;
// test/util/tile_cover.test.cpp: needs the camera's transform.
mod tile_cover_test;
// test/tile/geojson_tile.test.cpp and the GeoJSON parts of
// test/style/source.test.cpp: need the camera's transform.
mod geojson_tile_test;
mod source_test;
// The terrain's drapes: their projection, their stencil, the tiles a drape
// draws, what draws one again, and their byte cap.
mod drape_reuse_test;
mod drape_test;
// Port-only: the renderer frees what it lets go of on its workers.
mod drop_on_workers_test;
// Port-only: malformed vector tiles become tile errors, not panics.
mod malformed_tile_test;
// The context destroys what it released after what was made of it.
mod release_test;
// Port-only: a tile drives its layout jobs.
mod tile_jobs_test;
// Port-only: a tile's layout in two stages, and the worker over them.
mod tile_layout_test;
// Port-only: the tile store keeps a tile's data, work and contents apart.
mod tile_store_test;
// Port-only: whether terrain draws, and its cover.
mod terrain_test;
// The VectorTile tests of test/tile/vector_tile.test.cpp.
mod vector_tile_test;

/// A pool for a test's background work.
pub(crate) fn pool() -> crate::exec::pool::Pool {
    crate::exec::pool::Pool::new(4)
}

/// Port-only: a tile store whose jobs run on `workers` and which frees and
/// parses glyphs on `pool`, with no images and no sprite loaded.
pub(crate) fn tile_store(
    pool: &crate::exec::pool::TaskGroup,
    workers: Box<dyn crate::worker::tile_workers::TileWorkers>,
) -> crate::tile_store::store::TileStore {
    let glyphs = crate::text::glyph_store::GlyphStore::new(
        crate::text::local_fonts::LocalGlyphs::default(),
        std::sync::Arc::default(),
        pool.clone(),
        crate::text::font_face_engine::default_engine(),
        std::sync::Arc::new(|| {}),
    );
    crate::tile_store::store::TileStore::new(
        workers,
        glyphs,
        crate::image::image_store::ImageStore::default(),
        pool.clone(),
        std::sync::Arc::new(|| {}),
    )
}

/// Port-only: a tile store whose jobs run on `pool`.
pub(crate) fn pool_store(pool: &crate::exec::pool::TaskGroup) -> crate::tile_store::store::TileStore {
    tile_store(pool, Box::new(crate::worker::tile_workers::PoolWorkers::new(pool.clone(), 8)))
}

/// Port-only: one update of `store`'s tiles, as the renderer's: gives the
/// images asked for their update, takes in what arrived and starts the next
/// jobs. Returns whether a tile of a source with synchronous updates laid
/// out.
pub(crate) fn update(store: &mut crate::tile_store::store::TileStore) -> bool {
    store.images_mut().begin_update();
    store.receive();
    store.drive()
}

/// Port-only: an origin whose tiles load nothing themselves: the test gives
/// them their data, at once or as a reply to a load it began, which
/// delivers the item it holds. Its tiles cover 512-pixel tiles of every
/// zoom.
pub(crate) struct Given<K>(std::marker::PhantomData<fn() -> K>);

impl<K: crate::source::TileKind> crate::source::DataOrigin for Given<K>
where
    K::Item: Send,
{
    type Data = K;
    type Tile = ();
    type Reply = Option<K::Item>;

    fn update(&mut self, _: &mut crate::source::OriginUpdate<'_, Self>) -> Option<crate::source::Cover> {
        Some(given_cover())
    }

    fn start(&mut self, _: &mut crate::source::LoadContext<'_, '_, Self>, _: Option<crate::source::WorldCopy<'_, ()>>) {
    }

    fn receive(&mut self, (): &mut (), item: Option<K::Item>, cx: &mut crate::source::LoadContext<'_, '_, Self>) {
        cx.deliver(item);
    }
}

/// What a [`Given`] origin's tiles cover.
fn given_cover() -> crate::source::Cover {
    crate::source::Cover {
        tile_size: mln_style::constants::TILE_SIZE_I,
        zoom_range: mln_style::range::Range::new(0, 22),
        bounds: None,
    }
}

/// Port-only: a [`Given`] origin of tiles of `kind`.
pub(crate) fn given(kind: mln_style::layer::SourceKind) -> Box<dyn crate::source::erased::ErasedOrigin> {
    use mln_style::layer::SourceKind;
    match kind {
        SourceKind::Geometry | SourceKind::None => {
            Box::new(Given::<crate::source::GeometryTiles>(std::marker::PhantomData))
        }
        SourceKind::Raster => Box::new(Given::<crate::source::RasterTiles>(std::marker::PhantomData)),
        SourceKind::RasterDem => Box::new(Given::<crate::source::DemTiles>(std::marker::PhantomData)),
    }
}

/// Port-only: the origin that the built-in type of `source` supplies.
pub(crate) fn origin_of(source: &mln_style::Source) -> Box<dyn crate::source::erased::ErasedOrigin> {
    let registry = crate::Registry::with_builtins();
    let source_type = registry.source_type(source.info().name).expect("a built-in source type");
    match source_type.supply(source) {
        crate::source::Supplied::Tiles(origin) => origin,
        crate::source::Supplied::Image(_) => panic!("a source with tiles"),
    }
}

/// Port-only: how the built-in type of `source`, an image source, reads its
/// image.
pub(crate) fn image_of(source: &mln_style::Source) -> crate::source::ImageOf {
    let registry = crate::Registry::with_builtins();
    let source_type = registry.source_type(source.info().name).expect("a built-in source type");
    match source_type.supply(source) {
        crate::source::Supplied::Image(image_of) => image_of,
        crate::source::Supplied::Tiles(_) => panic!("an image source"),
    }
}

/// Port-only: the tileset origin's state of `tile`, a tile of a tileset
/// source.
pub(crate) fn tileset_tile(tile: &crate::tile_store::tile::Tile) -> &crate::source::tileset::TilesetTile {
    let state = tile.load.state.as_deref().expect("the origin's state of the tile");
    state.downcast_ref().expect("a tileset tile")
}

/// Port-only: what a test's map says of its tiles: drawn for `mode`, loaded
/// through `file_source`.
pub(crate) fn map_settings(
    mode: crate::map::MapMode,
    file_source: Option<std::sync::Arc<dyn crate::storage::FileSource>>,
) -> crate::source::MapSettings {
    crate::source::MapSettings { mode, pixel_ratio: 1.0, max_zoom: 22.0, file_source, fast_pfor_enabled: false }
}

/// Port-only: one tile of a source of its own in a store, driven as the
/// renderer drives it, whose data the test gives it.
pub(crate) struct OneTile {
    pub(crate) store: crate::tile_store::store::TileStore,
    pub(crate) key: crate::worker::job::SourceKey,
    pub(crate) id: crate::tile::OverscaledTileID,
    mode: crate::map::MapMode,
}

impl OneTile {
    /// Tile `id` of kind `kind` in `store`, laid out for `mode`.
    pub(crate) fn new(
        mut store: crate::tile_store::store::TileStore,
        kind: mln_style::layer::SourceKind,
        id: crate::tile::OverscaledTileID,
        mode: crate::map::MapMode,
    ) -> Self {
        let key = store.add_source("s".to_owned(), given(kind));
        let (source, _) = store.source_mut(key).expect("the source");
        source.info.cover = Some(given_cover());
        let mut tiles = OneTile { store, key, id, mode };
        tiles.add(id);
        tiles
    }

    fn params(&self) -> crate::layer::tile_layout::LayoutParams {
        crate::layer::tile_layout::LayoutParams {
            mode: self.mode,
            pixel_ratio: 1.0,
            subdivision_granularity: mln_style::subdivision_granularity::SubdivisionGranularitySetting::none(),
            first_load: true,
        }
    }

    /// Tiles `ids` of `source`, in `store`, whose data comes from the
    /// origin its built-in type supplies, loaded through `file_source` and
    /// laid out for `mode`. The origin takes in `source`, then the tiles'
    /// loads start as a cover starts them.
    pub(crate) fn with_origin(
        mut store: crate::tile_store::store::TileStore,
        source: &mln_style::Source,
        file_source: Option<std::sync::Arc<dyn crate::storage::FileSource>>,
        ids: &[crate::tile::OverscaledTileID],
        mode: crate::map::MapMode,
    ) -> Self {
        let key = store.add_source("source".to_owned(), origin_of(source));
        let mut tiles = OneTile { store, key, id: ids[0], mode };
        let params = tiles.params();
        let (tiles_of_source, ctx) = tiles.store.source_mut(key).expect("the source");
        let cover = tiles_of_source.update_origin(source, ctx, &map_settings(mode, file_source), false);
        assert!(cover.is_some(), "the source has tiles");
        for id in ids {
            tiles_of_source.add_tile(*id, params, false, ctx).expect("a tile");
        }
        tiles
    }

    /// Sets how much tile `id` is needed.
    pub(crate) fn set_necessity(
        &mut self,
        id: &crate::tile::OverscaledTileID,
        necessity: crate::tile_store::tile::TileNecessity,
    ) {
        self.with_at(id, |tile| tile.set_necessity(necessity));
    }

    /// How many changes the store reported since the last call, after it
    /// took in what arrived.
    pub(crate) fn changes(&mut self) -> usize {
        self.store.receive();
        self.store
            .take_events()
            .into_iter()
            .filter(|event| matches!(event, crate::tile_store::store::StoreEvent::Changed))
            .count()
    }

    /// Adds another tile, `id`, to the source, which loads nothing itself.
    pub(crate) fn add(&mut self, id: crate::tile::OverscaledTileID) {
        let params = self.params();
        let (source, _) = self.store.source_mut(self.key).expect("the source");
        let mut tile = source.info.new_tile(id, params).expect("a tile");
        tile.load.state = Some(Box::new(()));
        source.insert(tile);
    }

    pub(crate) fn tile(&self) -> &crate::tile_store::tile::Tile {
        self.tile_at(&self.id)
    }

    /// The layout parameters of the tile.
    pub(crate) fn tile_params(&self) -> crate::layer::tile_layout::LayoutParams {
        self.layout().params
    }

    /// The layout of the tile, a geometry tile.
    pub(crate) fn layout(&self) -> &crate::tile_store::pipeline::Layout {
        self.tile().pipeline.layout().expect("a geometry tile")
    }

    pub(crate) fn tile_at(&self, id: &crate::tile::OverscaledTileID) -> &crate::tile_store::tile::Tile {
        self.store.source(self.key).and_then(|source| source.tile(id)).expect("the tile")
    }

    /// Changes the tile through `f`.
    pub(crate) fn with<R>(&mut self, f: impl FnOnce(&mut crate::tile_store::tile_mut::TileMut<'_>) -> R) -> R {
        let id = self.id;
        self.with_at(&id, f)
    }

    /// Changes tile `id` through `f`.
    pub(crate) fn with_at<R>(
        &mut self,
        id: &crate::tile::OverscaledTileID,
        f: impl FnOnce(&mut crate::tile_store::tile_mut::TileMut<'_>) -> R,
    ) -> R {
        let (source, ctx) = self.store.source_mut(self.key).expect("the source");
        source.update(id, ctx, f).expect("the tile")
    }

    pub(crate) fn set_style(&mut self, style: &std::sync::Arc<crate::worker::job::SourceStyle>) {
        let params = self.params();
        self.with(|tile| tile.set_style(style, params.pixel_ratio, params.subdivision_granularity));
    }

    /// Gives a geometry tile `data`, as a load of its own.
    pub(crate) fn set_data(&mut self, data: Option<crate::tile::TileData>) {
        self.with(|tile| tile.take_data(data, false));
    }

    /// Gives raster tile `id` the image `data`, as a load of its own.
    pub(crate) fn set_image(&mut self, id: &crate::tile::OverscaledTileID, data: Option<crate::storage::Data>) {
        self.with_at(id, |tile| tile.take_image(data, false));
    }

    /// Gives tile `id` no data, as a load that finds the tileset has no
    /// tile there.
    pub(crate) fn set_missing(&mut self, id: &crate::tile::OverscaledTileID) {
        self.with_at(id, |tile| {
            tile.take_missing();
        });
    }

    /// What the store reported its tiles went through since the last call.
    pub(crate) fn actions(&mut self) -> Vec<crate::tile_store::tile::TileOperation> {
        self.store
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                crate::tile_store::store::StoreEvent::Action { op, .. } => Some(op),
                crate::tile_store::store::StoreEvent::Changed | crate::tile_store::store::StoreEvent::Error { .. } => {
                    None
                }
            })
            .collect()
    }

    /// The errors the store reported since the last call.
    pub(crate) fn errors(&mut self) -> Vec<String> {
        self.store
            .take_events()
            .into_iter()
            .filter_map(|event| match event {
                crate::tile_store::store::StoreEvent::Error { error, .. } => Some(error.to_string()),
                crate::tile_store::store::StoreEvent::Changed | crate::tile_store::store::StoreEvent::Action { .. } => {
                    None
                }
            })
            .collect()
    }

    /// One update of the store.
    pub(crate) fn update(&mut self) -> bool {
        update(&mut self.store)
    }

    /// Updates the store until the tiles `ids` are all complete, updating at
    /// least once.
    ///
    /// # Panics
    ///
    /// If they don't complete within 30 seconds.
    pub(crate) fn settle(&mut self, ids: &[crate::tile::OverscaledTileID]) {
        let deadline = mln_style::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            self.update();
            if ids.iter().all(|id| self.tile_at(id).is_complete()) {
                return;
            }
            assert!(mln_style::time::Instant::now() < deadline, "the tiles didn't complete");
            std::thread::yield_now();
        }
    }

    /// Whether the tile shows a layout.
    pub(crate) fn shows_layout(&self) -> bool {
        self.tile().contents().and_then(crate::tile_store::contents::Contents::geometry).is_some()
    }

    /// Lets go of the tile, as its source does when it leaves the cache.
    pub(crate) fn drop_tile(&mut self) {
        let id = self.id;
        self.drop_tile_at(&id);
    }

    /// Lets go of tile `id`.
    pub(crate) fn drop_tile_at(&mut self, id: &crate::tile::OverscaledTileID) {
        let (source, ctx) = self.store.source_mut(self.key).expect("the source");
        source.release(id, ctx);
    }
}

/// Port-only: a source's style of `layers`, without global state or font
/// faces.
pub(crate) fn style(
    layers: &[std::sync::Arc<dyn crate::layer::layer_type::ErasedEvaluatedLayer>],
) -> std::sync::Arc<crate::worker::job::SourceStyle> {
    std::sync::Arc::new(crate::worker::job::SourceStyle {
        version: 0,
        layers: layers.to_vec(),
        global_state: None,
        font_faces: None,
        glyph_pbfs: true,
    })
}

/// Port-only: the render layer of the layer `style`, of a built-in type.
pub(crate) fn render_layer(
    style: &std::sync::Arc<dyn mln_style::layer::ErasedLayerStyle>,
) -> Box<dyn crate::layer::render_layer::RenderLayer> {
    let registry = crate::Registry::with_builtins();
    let layer_type = registry.layer_type(style.info().name).expect("a built-in layer type");
    layer_type.create_render_layer(style).expect("a layer of its type")
}

/// The render source of `source`, through the built-in registry.
pub(crate) fn create_render_source(
    source: &mln_style::Source,
    store: &mut crate::tile_store::store::TileStore,
) -> Box<dyn crate::source::render_source::RenderSource> {
    crate::source::render_source::create(&crate::registry::Registry::with_builtins(), source, store)
        .expect("a built-in source type")
}

/// `source` with the description that its type knows without a fetch, such
/// as the tileset a style lists, as the map's loader gives it.
pub(crate) fn ready(source: mln_style::Source) -> mln_style::Source {
    match source.data().plan_load() {
        mln_style::source::Load::Ready(description) => {
            source.with_description(Some(description)).expect("a description of the source's type")
        }
        _ => source,
    }
}

/// A WebGPU device, or `None` where no adapter is available: the test is
/// skipped there, unless `MLN_RENDER_BACKEND` asks for `webgpu`.
pub(super) fn webgpu_device() -> Option<mln_webgpu::Device> {
    let requested = std::env::var("MLN_RENDER_BACKEND").is_ok_and(|backend| backend.eq_ignore_ascii_case("webgpu"));
    match mln_webgpu::Device::new(None) {
        Ok(device) => Some(device),
        Err(error) if error.kind == mln_gfx::ErrorKind::Unsupported && !requested => {
            eprintln!("skipped: {error}");
            None
        }
        Err(error) => panic!("WebGPU device: {error}"),
    }
}

/// Runs `f` with the `Gpu` of a headless WebGPU backend and a context over
/// it, which it then tears down. Skipped where no WebGPU adapter is
/// available unless `MLN_RENDER_BACKEND` asks for `webgpu`.
fn with_context(f: impl FnOnce(&mut dyn mln_gfx::gpu::Gpu, &mut crate::draw::context::Context)) {
    use mln_gfx::RendererBackend as _;
    use mln_gfx::gpu::Gpu as _;
    let Some(device) = webgpu_device() else {
        return;
    };
    let mut backend = mln_webgpu::RendererBackend::new(&device);
    let gpu = backend.gpu();
    let mut context = crate::draw::context::Context::new(*gpu.caps(), 1.0);
    f(gpu, &mut context);
    context.tear_down(gpu);
}
