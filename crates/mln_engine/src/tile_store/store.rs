//! The tile store owns every tiled source's tiles, the workers their jobs
//! run on, the glyph and image stores and atlas their dependencies resolve
//! through, and the inbox where what arrives for them waits.
//!
//! A tile's job lays out its data, or decodes its image, for one generation
//! of its inputs. Each change of an input takes a new generation from the
//! source's counter, and a reply applies only if it names the current one.
//! A job that answers `NeedsDeps` waits until the stores have its glyphs and
//! images, packed into the atlas, and runs again with their positions.

use std::rc::Rc;
use std::sync::Arc;

use mln_style::error::Error;
use mln_style::hash::{HashMap, HashSet};
use mln_style::layer::SourceKind;
use mln_style::tileset::RasterEncoding;

use crate::draw::context::Context;
use crate::draw::dynamic_texture_atlas::DynamicTextureAtlas;
use crate::draw::identity::SimpleIdentity;
use crate::exec::pool::TaskGroup;
use crate::image::image_store::ImageStore;
use crate::layer::bucket::ErasedBucket;
use crate::layer::tile_layout::{LayoutParams, Resolved};
use crate::placement::{BucketKey, LaidBuckets};
use crate::render::events::RendererEvent;
use crate::render::orchestrator::drop_on_workers;
use crate::render::tile::RenderTile;
use crate::source::Cover;
use crate::source::erased::ErasedOrigin;
use crate::source::source_state::SourceFeatureState;
use crate::storage::FileSource;
use crate::text::glyph_store::GlyphStore;
use crate::tile::OverscaledTileID;
use crate::tile_store::contents::{Answer, TileAtlases};
use crate::tile_store::inbox::{Inbox, InboxSender, TileEvent, Waker};
use crate::tile_store::loading::Load;
use crate::tile_store::pipeline::{Pipeline, Resume, Waiting};
use crate::tile_store::source_tiles::{Slot, SourceTiles};
use crate::tile_store::tile::{Tile, TileLoadOrder, TileOperation};
use crate::worker::job::{SourceKey, TileKey};
use crate::worker::tile_workers::{Jobs, TileWorkers};

#[derive(Debug)]
pub(crate) enum StoreEvent {
    /// A tile changed: the renderer needs a frame to show it.
    Changed,
    Error {
        source: String,
        tile: OverscaledTileID,
        error: Error,
    },
    Action {
        op: TileOperation,
        tile: OverscaledTileID,
        source: String,
    },
}

/// The parts of the store a source's tiles work with.
pub(crate) struct StoreContext {
    pub(crate) inbox: InboxSender,
    pub(crate) jobs: Jobs,
    pub(crate) glyphs: GlyphStore,
    pub(crate) images: ImageStore,
    pub(crate) atlas: DynamicTextureAtlas,
    /// The renderer's workers, which free what tiles let go of.
    pub(crate) pool: TaskGroup,
    /// Where glyphs load from.
    pub(crate) file_source: Option<Arc<dyn FileSource>>,
    events: Vec<StoreEvent>,
    /// What the glyph and image stores reported, in the order they did.
    resource_events: Vec<RendererEvent>,
}

impl StoreContext {
    pub(crate) fn changed(&mut self) {
        self.events.push(StoreEvent::Changed);
    }

    pub(crate) fn error(&mut self, source: &str, tile: OverscaledTileID, error: Error) {
        self.events.push(StoreEvent::Error { source: source.to_owned(), tile, error });
    }

    pub(crate) fn action(&mut self, source: &str, tile: OverscaledTileID, op: TileOperation) {
        self.events.push(StoreEvent::Action { op, tile, source: source.to_owned() });
    }

    /// Takes what the glyph and image stores reported, after a call to one
    /// of them, so that their events keep the order they happened in.
    fn collect_resource_events(&mut self) {
        self.resource_events.extend(self.glyphs.take_events());
        self.resource_events.extend(self.images.take_events());
    }

    /// The resume of `waiting`'s job, once the stores have all it needs or
    /// know them missing: packed into the atlas.
    pub(crate) fn resolve(&mut self, waiting: &mut Waiting) -> Option<Resume> {
        let Waiting { lane, needs, wait, images: seen } = waiting;
        let revisions = (self.glyphs.revision(), self.images.revision());
        if wait.revisions == Some(revisions) {
            return None;
        }
        let glyphs = self.glyphs.resolve(&needs.glyphs, &mut wait.glyphs, self.file_source.as_ref(), &self.inbox);
        self.collect_resource_events();
        let images = self.images.resolve(&needs.images, &mut wait.images);
        self.collect_resource_events();
        let (Some(glyphs), Some(images)) = (glyphs, images) else {
            wait.revisions = Some((self.glyphs.revision(), self.images.revision()));
            return None;
        };

        let glyph_page = self.atlas.pack_glyphs(&glyphs.glyphs);
        let image_page = self.atlas.pack_images(&images.icons, &images.patterns);
        let missing_images = needs
            .images
            .keys()
            .filter(|id| !images.icons.contains_key(*id) && !images.patterns.contains_key(*id))
            .cloned()
            .collect();
        let atlases = TileAtlases {
            glyphs: glyph_page.lease,
            images: image_page.lease,
            pattern_positions: image_page.patterns.clone(),
        };
        let resolved = Resolved {
            glyphs: glyph_page.positions,
            shaping: glyphs.shaping,
            icons: image_page.icons,
            patterns: image_page.patterns,
            missing_images,
        };
        let answer =
            Answer { resolved: Arc::new(resolved), atlases: Some(Rc::new(atlases)), failed_glyphs: glyphs.failed };
        Some(Resume { lane: Some(*lane), known: std::mem::take(needs), answer, images: Arc::clone(seen) })
    }
}

/// What a source's tiles share.
pub(crate) struct SourceInfo {
    pub(crate) key: SourceKey,
    pub(crate) id: String,
    /// The kind of data the origin delivers.
    pub(crate) kind: SourceKind,
    /// Where the tiles get their data; `None` only while one of its hooks
    /// has it.
    pub(crate) origin: Option<Box<dyn ErasedOrigin>>,
    /// What the tiles covered as the origin last said, or `None` while it
    /// has nothing to make tiles from.
    pub(crate) cover: Option<Cover>,
    /// How a raster-dem source's pixels encode elevations, as its origin
    /// last set it.
    pub(crate) dem_encoding: RasterEncoding,
    /// Grows each time the tiles are all let go of: those after are of
    /// another version of the source's data.
    pub(crate) data_version: u64,
    /// Loads and generations share it, so no two share a value.
    counter: u64,
    /// The tiles' jobs run on the render thread as soon as they're ready.
    pub(crate) synchronous: bool,
    pub(crate) feature_state: SourceFeatureState,
}

impl SourceInfo {
    pub(crate) fn new(key: SourceKey, id: String, origin: Box<dyn ErasedOrigin>) -> Self {
        SourceInfo {
            key,
            id,
            kind: origin.kind(),
            origin: Some(origin),
            cover: None,
            dem_encoding: RasterEncoding::Mapbox,
            data_version: 0,
            counter: 0,
            synchronous: false,
            feature_state: SourceFeatureState::new(),
        }
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    /// A new tile `id`, which its origin has yet to start; `None` while the
    /// source has nothing to make tiles from.
    pub(crate) fn new_tile(&mut self, id: OverscaledTileID, params: LayoutParams) -> Option<Tile> {
        self.cover?;
        let generation = self.next();
        Some(Tile::new(id, Load::new(), Pipeline::new(self.kind, self.dem_encoding, generation, params)))
    }

    pub(crate) fn tile_key(&self, tile: &Tile) -> TileKey {
        TileKey { source: self.key, id: tile.id }
    }
}

/// The source of `key` and the id of its tile `is` holds for: `key`'s, or
/// one that moved to another wrap since.
fn find(
    sources: &mut HashMap<SourceKey, SourceTiles>,
    key: TileKey,
    is: impl Fn(&Tile) -> bool,
) -> Option<(&mut SourceTiles, OverscaledTileID)> {
    let source = sources.get_mut(&key.source)?;
    if source.tile(&key.id).is_some_and(&is) {
        return Some((source, key.id));
    }
    let id = source.slots().find(|slot| is(&slot.tile))?.tile.id;
    Some((source, id))
}

pub struct TileStore {
    sources: HashMap<SourceKey, SourceTiles>,
    /// The sources in the style's order, which orders the tiles' jobs within
    /// a load order.
    order: Vec<SourceKey>,
    next_source: u64,
    cache_enabled: bool,
    inbox: Inbox,
    ctx: StoreContext,
}

impl TileStore {
    /// A store whose jobs run on `workers`, which frees on `pool` and calls
    /// `wake` when something arrives for its tiles.
    pub(crate) fn new(
        workers: Box<dyn TileWorkers>,
        glyphs: GlyphStore,
        images: ImageStore,
        pool: TaskGroup,
        wake: Waker,
    ) -> Self {
        let inbox = Inbox::new(wake);
        let ctx = StoreContext {
            inbox: inbox.sender().clone(),
            jobs: Jobs::new(workers, inbox.sender()),
            glyphs,
            images,
            atlas: DynamicTextureAtlas::new(),
            pool,
            file_source: None,
            events: Vec::new(),
            resource_events: Vec::new(),
        };
        TileStore { sources: HashMap::default(), order: Vec::new(), next_source: 0, cache_enabled: true, inbox, ctx }
    }

    pub(crate) fn glyphs(&mut self) -> &mut GlyphStore {
        &mut self.ctx.glyphs
    }

    pub(crate) fn images(&self) -> &ImageStore {
        &self.ctx.images
    }

    pub(crate) fn images_mut(&mut self) -> &mut ImageStore {
        &mut self.ctx.images
    }

    pub(crate) fn atlas(&mut self) -> &mut DynamicTextureAtlas {
        &mut self.ctx.atlas
    }

    #[cfg(test)]
    pub(crate) fn inbox(&self) -> &InboxSender {
        &self.ctx.inbox
    }

    pub(crate) fn set_file_source(&mut self, file_source: Option<Arc<dyn FileSource>>) {
        self.ctx.file_source = file_source;
    }

    /// Runs the tiles' jobs in `workers`, where those in flight run again.
    pub(crate) fn set_workers(&mut self, workers: Box<dyn TileWorkers>) {
        self.ctx.jobs.set_workers(workers, &self.ctx.inbox);
        for source in self.sources.values_mut() {
            source.update_each(
                &mut self.ctx,
                |slot| slot.tile.pipeline.lane().is_some(),
                |tile| {
                    if let Some(lane) = tile.tile().pipeline.lane() {
                        tile.lane_lost(lane);
                    }
                },
            );
        }
    }

    /// Adds a source after the others in the style's order, whose tiles get
    /// their data from `origin`.
    pub(crate) fn add_source(&mut self, id: String, origin: Box<dyn ErasedOrigin>) -> SourceKey {
        self.next_source += 1;
        let key = SourceKey(self.next_source);
        let mut source = SourceTiles::new(SourceInfo::new(key, id, origin));
        source.set_cache_enabled(self.cache_enabled);
        self.sources.insert(key, source);
        self.order.push(key);
        key
    }

    pub(crate) fn remove_source(&mut self, key: SourceKey) {
        if let Some(mut source) = self.sources.remove(&key) {
            source.clear_all(&mut self.ctx);
        }
        self.order.retain(|other| *other != key);
    }

    pub(crate) fn clear(&mut self) {
        for key in std::mem::take(&mut self.order) {
            self.remove_source(key);
        }
    }

    /// The sources' tiles, in the style's order.
    #[cfg(feature = "bench")]
    pub(crate) fn sources(&self) -> impl Iterator<Item = &SourceTiles> + '_ {
        self.order.iter().filter_map(|key| self.sources.get(key))
    }

    pub(crate) fn source(&self, key: SourceKey) -> Option<&SourceTiles> {
        self.sources.get(&key)
    }

    pub(crate) fn source_mut(&mut self, key: SourceKey) -> Option<(&mut SourceTiles, &mut StoreContext)> {
        Some((self.sources.get_mut(&key)?, &mut self.ctx))
    }

    /// Readies the buckets of the tiles of `render_tiles`, render tiles of
    /// the source `key`, for the frame's draws (`SourceTiles::upload_at`).
    pub(crate) fn upload(&mut self, key: SourceKey, render_tiles: &[RenderTile], context: &mut Context) {
        let Some(tiles) = self.sources.get_mut(&key) else {
            return;
        };
        for render_tile in render_tiles {
            tiles.upload_at(render_tile.overscaled_tile_id(), render_tile.slot(), &self.ctx.atlas, context);
        }
    }

    pub(crate) fn take_events(&mut self) -> Vec<StoreEvent> {
        std::mem::take(&mut self.ctx.events)
    }

    /// What the glyph and image stores reported since the last call, oldest
    /// first.
    pub(crate) fn take_resource_events(&mut self) -> Vec<RendererEvent> {
        self.ctx.collect_resource_events();
        std::mem::take(&mut self.ctx.resource_events)
    }

    /// Takes in what arrived for the tiles.
    pub(crate) fn receive(&mut self) {
        self.ctx.glyphs.update();
        self.ctx.collect_resource_events();
        while let Some(event) = self.inbox.next() {
            self.take(event);
        }
        self.glyphs_recovered();
    }

    /// Lays out again the tiles that lacked glyphs of a range that has
    /// loaded since it failed.
    fn glyphs_recovered(&mut self) {
        let recovered = self.ctx.glyphs.take_recovered();
        if recovered.is_empty() {
            return;
        }
        for key in &self.order {
            if let Some(source) = self.sources.get_mut(key) {
                source.update_each(&mut self.ctx, |_| true, |tile| tile.glyphs_recovered(&recovered));
            }
        }
    }

    /// Takes in one event for the tiles.
    fn take(&mut self, event: TileEvent) {
        match event {
            TileEvent::Load { tile, load, reply } => {
                if let Some((source, tile)) = find(&mut self.sources, tile, |tile| tile.load.awaits(load)) {
                    source.update(&tile, &mut self.ctx, |tile| tile.receive(reply));
                }
            }
            TileEvent::Reply { lane, id, body, times } => {
                if !self.ctx.jobs.replied(&id, times) {
                    drop_on_workers(&self.ctx.pool, body);
                    return;
                }
                let running = |tile: &Tile| tile.pipeline.in_flight().is_some_and(|job| job.id == id);
                match find(&mut self.sources, id.tile, running) {
                    Some((source, tile)) => {
                        source.update(&tile, &mut self.ctx, |tile| tile.apply_reply(lane, id, body));
                    }
                    None => drop_on_workers(&self.ctx.pool, body),
                }
            }
            TileEvent::LaneLost(lane) => {
                self.ctx.jobs.lost(lane);
                for source in self.sources.values_mut() {
                    let holds = |slot: &Slot| slot.tile.pipeline.lane() == Some(lane);
                    source.update_each(&mut self.ctx, holds, |tile| tile.lane_lost(lane));
                }
            }
            TileEvent::Remote(event) => {
                if let Some(event) = self.ctx.jobs.receive(event) {
                    self.take(event);
                }
            }
            TileEvent::Glyphs { font_stack, range, id, result } => {
                self.ctx.glyphs.take_response(&font_stack, range, id, result);
                self.ctx.collect_resource_events();
            }
            TileEvent::DeviceFonts { font_stack, faces } => {
                self.ctx.glyphs.take_device_fonts(&font_stack, faces);
                self.ctx.collect_resource_events();
            }
        }
    }

    /// Moves the tiles' work on, the view's first. Returns whether a tile of
    /// a synchronous source laid out.
    pub(crate) fn drive(&mut self) -> bool {
        let mut tiles: Vec<(TileLoadOrder, usize, OverscaledTileID)> = Vec::new();
        for (index, key) in self.order.iter().enumerate() {
            if let Some(source) = self.sources.get(key) {
                tiles.extend(
                    source
                        .slots()
                        .filter(|slot| slot.tile.has_work())
                        .map(|slot| (slot.place().order(), index, slot.tile.id)),
                );
            }
        }
        tiles.sort_by_key(|(order, index, _)| (*order, *index));
        let mut laid_out = false;
        for (_, index, id) in tiles {
            let Some(source) = self.sources.get_mut(&self.order[index]) else {
                continue;
            };
            laid_out |= source
                .update(&id, &mut self.ctx, |tile| {
                    tile.drive();
                    tile.installed
                })
                .unwrap_or(false);
        }
        laid_out
    }

    /// Lays out again the tiles that the images in `changed`, added, removed
    /// or resized, touch.
    pub(crate) fn images_changed(&mut self, changed: &HashSet<String>) {
        let available = Arc::clone(self.ctx.images.available_images());
        for key in &self.order {
            if let Some(source) = self.sources.get_mut(key) {
                source.update_each(&mut self.ctx, |_| true, |tile| tile.images_changed(changed, &available));
            }
        }
    }

    /// Retries the failed glyph ranges and tile loads.
    pub(crate) fn retry_failed(&mut self) {
        for key in &self.order {
            if let Some(source) = self.sources.get_mut(key) {
                // Not the methods' paths: those name one lifetime of `TileMut`.
                source.update_each(
                    &mut self.ctx,
                    |_| true,
                    |tile| {
                        tile.retry_failed_glyphs();
                    },
                );
                source.update_each(
                    &mut self.ctx,
                    |slot| !slot.place().is_cached(),
                    |tile| {
                        tile.retry_failed_load();
                    },
                );
            }
        }
    }

    pub(crate) fn images_used(&self) -> HashSet<String> {
        let mut used = HashSet::default();
        for source in self.sources.values() {
            for slot in source.slots() {
                slot.tile.dependencies(|deps| used.extend(deps.images.keys().cloned()));
            }
        }
        used
    }

    pub(crate) fn has_fading_tiles(&self) -> bool {
        self.sources.values().any(SourceTiles::has_fading_tiles)
    }

    pub(crate) fn set_cache_enabled(&mut self, enable: bool) {
        self.cache_enabled = enable;
        for source in self.sources.values_mut() {
            source.set_cache_enabled(enable);
        }
    }

    pub(crate) fn reduce_memory_use(&mut self) {
        for source in self.sources.values_mut() {
            source.reduce_memory_use(&mut self.ctx);
        }
    }

    pub(crate) fn dump_debug_logs(&self) {
        for source in self.sources.values() {
            source.dump_debug_logs();
        }
        self.ctx.images.dump_debug_logs();
        self.ctx.jobs.log_stats();
    }

    #[cfg(test)]
    pub(crate) fn is_idle(&self) -> bool {
        self.ctx.jobs.is_idle()
    }
}

/// The buckets of the tiles' layouts.
impl LaidBuckets for TileStore {
    fn id(&self, key: &BucketKey) -> Option<SimpleIdentity> {
        Some(self.bucket(key)?.id())
    }

    fn bucket(&self, key: &BucketKey) -> Option<&dyn ErasedBucket> {
        let contents = self.sources.get(&key.source)?.contents_at(&key.tile, key.slot)?.geometry()?;
        contents.result.layer_render_data.bucket(key.bucket)
    }

    fn bucket_mut(&mut self, key: &BucketKey) -> Option<&mut dyn ErasedBucket> {
        let contents = self.sources.get_mut(&key.source)?.contents_at_mut(&key.tile, key.slot)?.geometry_mut()?;
        contents.result.layer_render_data.bucket_mut(key.bucket)
    }
}
