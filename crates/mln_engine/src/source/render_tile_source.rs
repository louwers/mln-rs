use std::sync::Arc;

use glam::Vec2;
use mln_style::Source;
use mln_style::error::Error;

use crate::draw::context::Context;
use crate::layer::bucket::UploadNeeds;
use crate::layer::layer_type::ErasedEvaluatedLayer;
use crate::query::FeatureExtensionValue;
use crate::render::tile::RenderTile;
use crate::source::render_source::{RenderSource, RenderSourceBase, RenderTileParameters, TileOrder};
use crate::source::source_type::{OriginUpdate, TileOrigin};
use crate::tile_store::source_tiles::{CoverUpdate, SourceTiles};
use crate::tile_store::store::TileStore;
use crate::tile_store::tile::TileKind;
use crate::tile_store::tile_parameters::TileParameters;
use crate::worker::job::SourceKey;

/// The render source of a source with tiles: a view into the tile store,
/// which keeps the tiles, with the render tiles of the last
/// `update_render_tiles`.
pub(crate) struct RenderTileSource {
    pub(crate) base: RenderSourceBase,
    key: SourceKey,
    origin: Box<dyn TileOrigin>,
    fast_pfor_enabled: bool,
    render_tiles: Vec<RenderTile>,
    /// The render tiles in [`TileOrder::ById`], as indexes into them.
    order_by_id: Vec<usize>,
    /// The render tiles in [`TileOrder::ByY`], where the frame's
    /// [`RenderTileParameters::sort_by_y`] asked for them; empty
    /// otherwise.
    order_by_y: Vec<usize>,
}

impl RenderTileSource {
    /// Sorts the render tiles into `order_by_y` in [`TileOrder::ByY`], seen
    /// at `bearing`: the deeper zooms first, then by rotated y and x.
    fn sort_by_y(&mut self, bearing: f32) {
        let tiles = &self.render_tiles;
        let rotation = Vec2::from_angle(bearing);
        let order = &mut self.order_by_y;
        order.extend(0..tiles.len());
        order.sort_by(|&a, &b| {
            let (a, b) = (&tiles[a].id.canonical, &tiles[b].id.canonical);
            let pa = rotation.rotate(Vec2::new(a.x as f32, a.y as f32));
            let pb = rotation.rotate(Vec2::new(b.x as f32, b.y as f32));
            b.z.cmp(&a.z).then(pa.y.total_cmp(&pb.y)).then(pa.x.total_cmp(&pb.x))
        });
    }

    /// The render source of `style_source`, whose tiles of `kind` go in `store`
    /// and get their data through `origin`. A tile of a `single_layer`
    /// source holds one source layer.
    pub(crate) fn new(
        style_source: Source,
        kind: TileKind,
        single_layer: bool,
        origin: Box<dyn TileOrigin>,
        store: &mut TileStore,
    ) -> Self {
        let key = store.add_source(style_source.id().to_owned(), kind, single_layer);
        RenderTileSource {
            base: RenderSourceBase::new(style_source),
            key,
            origin,
            fast_pfor_enabled: false,
            render_tiles: Vec::new(),
            order_by_id: Vec::new(),
            order_by_y: Vec::new(),
        }
    }
}

impl RenderSource for RenderTileSource {
    fn base(&self) -> &RenderSourceBase {
        &self.base
    }

    fn is_loaded(&self, store: &TileStore) -> bool {
        store.source(self.key).is_none_or(SourceTiles::is_loaded)
    }

    fn update(
        &mut self,
        store: &mut TileStore,
        style_source: Source,
        layers: &[Arc<dyn ErasedEvaluatedLayer>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    ) {
        let previous = std::mem::replace(&mut self.base.style_source, style_source);
        self.base.enabled = needs_rendering;
        let Some((source, ctx)) = store.source_mut(self.key) else {
            return;
        };
        let origin_update = OriginUpdate { parameters, needs_relayout, fast_pfor_enabled: self.fast_pfor_enabled };
        let Some(cover) = self.origin.update(&previous, &self.base.style_source, source, ctx, &origin_update) else {
            return;
        };
        let source_id = self.base.style_source.id();
        let terrain_reads = parameters.terrain.filter(|terrain| terrain.source_id == source_id);
        let update = CoverUpdate {
            layers,
            needs_rendering,
            needs_relayout,
            terrain_reads: terrain_reads.map_or(&[], |terrain| &terrain.dem_tiles),
            parameters,
            style_source: &self.base.style_source,
            tile_size: cover.tile_size,
            zoom_range: cover.zoom_range,
            bounds: cover.bounds,
        };
        source.update_cover(&update, ctx);
    }

    fn update_render_tiles(&mut self, store: &mut TileStore, parameters: &RenderTileParameters<'_>) {
        self.render_tiles.clear();
        if let Some(source) = store.source(self.key) {
            self.render_tiles.reserve(source.rendered_count());
            for rendered in source.rendered() {
                let mut render_tile =
                    RenderTile::new(*rendered.id, rendered.tile, rendered.slot, rendered.hold_for_fade);
                render_tile.set_projection(parameters.transform);
                self.render_tiles.push(render_tile);
            }
        }
        let tiles = &self.render_tiles;
        self.order_by_id.clear();
        self.order_by_id.extend((0..tiles.len()).filter(|&i| !tiles[i].hold_for_fade()));
        self.order_by_y.clear();
        if parameters.sort_by_y {
            self.sort_by_y(parameters.transform.view.angle() as f32);
        }
    }

    fn release_render_data(&mut self) {
        self.render_tiles = Vec::new();
        self.order_by_id = Vec::new();
        self.order_by_y = Vec::new();
    }

    fn upload(&mut self, store: &mut TileStore, context: &mut Context, needs: UploadNeeds) {
        store.upload(self.key, &self.render_tiles, context, needs);
    }

    fn tiles(&self) -> Option<SourceKey> {
        Some(self.key)
    }

    fn render_tiles(&self) -> &[RenderTile] {
        &self.render_tiles
    }

    fn tile_order(&self, order: TileOrder) -> Option<&[usize]> {
        Some(match order {
            TileOrder::ById => &self.order_by_id,
            TileOrder::ByY => &self.order_by_y,
        })
    }

    fn query_feature_extensions(
        &self,
        feature: &mln_util::feature::Feature,
        extension: &str,
        extension_field: &str,
        args: Option<&feature::PropertyMap>,
    ) -> Result<FeatureExtensionValue, Error> {
        self.origin.query_feature_extensions(feature, extension, extension_field, args)
    }

    fn set_fast_pfor_enabled(&mut self, enable: bool) {
        self.fast_pfor_enabled = enable;
    }

    fn dump_debug_logs(&self) {}

    fn max_zoom(&self) -> Option<u8> {
        Some(self.origin.max_zoom())
    }
}
