// A render source is the renderer's side of a style source: what every one
// has is `RenderSourceBase`, what each type does the `RenderSource` trait. A
// source's tiles are in the renderer's tile store, which the source's
// methods are given.

use std::sync::Arc;

use mln_map_types::MapDebugOptions;
use mln_style::Source;
use mln_style::layer_type::SourceKind;

use crate::erased_layer::ErasedEvaluatedLayer;
use crate::gfx::context::Resources;
use crate::paint_parameters::{PaintParameters, TransformParameters};
use crate::registry::Registry;
use crate::render_tile::RenderTile;
use crate::source::{Supplied, Supply, TileKind};
use crate::sources::render_image_source::RenderImageSource;
use crate::sources::render_tile_source::{DebugLayers, RenderTileSource};
use crate::tile::job::SourceKey;
use crate::tile::raster::RasterContents;
use crate::tile::store::TileStore;
use crate::tile_parameters::TileParameters;

pub(crate) struct RenderTileParameters<'a> {
    pub(crate) transform: &'a TransformParameters,
    pub(crate) debug_options: MapDebugOptions,
    pub(crate) source_name: String,
    /// Whether a layer of the frame draws the source's tiles in
    /// [`TileOrder::ByY`], which `update_render_tiles` then sorts them in.
    pub(crate) sort_by_y: bool,
}

/// What every render source has, whatever its type.
pub(crate) struct RenderSourceBase {
    pub(crate) style_source: Source,
    pub(crate) enabled: bool,
}

impl RenderSourceBase {
    pub(crate) fn new(style_source: Source) -> Self {
        RenderSourceBase { style_source, enabled: false }
    }
}

/// An order a layer draws its source's tiles in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TileOrder {
    /// By tile id, without the tiles held for fade.
    ById,
    /// By opposite y position, so that the lower of two tiles' overlapping
    /// contents draw over the higher's.
    ByY,
}

pub(crate) trait RenderSource {
    fn base(&self) -> &RenderSourceBase;

    fn is_loaded(&self, store: &TileStore) -> bool;

    fn update(
        &mut self,
        store: &mut TileStore,
        style_source: Source,
        layers: &[Arc<dyn ErasedEvaluatedLayer>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    );

    /// Makes the source's render tiles of the frame.
    fn update_render_tiles(&mut self, store: &mut TileStore, parameters: &RenderTileParameters<'_>);

    /// Readies the buckets the frame draws, before its layers make their
    /// draws, uploading their indexes through `context`.
    fn upload(&mut self, store: &mut TileStore, context: &mut Resources);

    /// Updates the source's draws of the debug overlays; a no-op for the
    /// sources that have none (the image source).
    fn update_debug_draws(&self, _debug_layers: &mut DebugLayers, _parameters: &mut PaintParameters<'_>) {}

    /// Releases the render data from the last `update_render_tiles` while
    /// the source is disabled.
    fn release_render_data(&mut self) {}

    /// The source's tiles in the tile store, for a source with tiles.
    fn tiles(&self) -> Option<SourceKey> {
        None
    }

    /// The source's render tiles, as of the last `update_render_tiles`.
    fn render_tiles(&self) -> &[RenderTile] {
        &[]
    }

    /// The source's tiles in `order`, as indexes into `render_tiles`, as of
    /// the last `update_render_tiles`; `None` for a source without tiles.
    fn tile_order(&self, _order: TileOrder) -> Option<&[usize]> {
        None
    }

    /// An image source's image, which each of its render tiles shows;
    /// `None` for other sources and before the image arrives.
    fn image(&self) -> Option<&RasterContents> {
        None
    }

    /// Whether the source is an image source, even before its image
    /// arrives.
    fn is_image(&self) -> bool {
        false
    }

    fn set_fast_pfor_enabled(&mut self, _enable: bool) {}

    fn dump_debug_logs(&self);

    /// The deepest zoom that has tiles of its own, as the source's last
    /// [`Cover`](crate::source::Cover) says; `None` for a source without
    /// tiles or before its first cover.
    fn max_zoom(&self) -> Option<u8>;

    fn is_enabled(&self) -> bool {
        self.base().enabled
    }
}

/// Creates the render source of a style source through its type in
/// `registry`, whose tiles, if it has any, `store` keeps. `None` for a type
/// that the registry lacks, which breaks the contract that a map and its
/// renderer register the same types.
pub(crate) fn create(registry: &Registry, source: &Source, store: &mut TileStore) -> Option<Box<dyn RenderSource>> {
    let name = source.info().name;
    let Some(source_type) = registry.source_type(name) else {
        debug_assert!(false, "the renderer's registry has no source type '{name}'");
        return None;
    };
    let Supply(supplied) = source_type.supply(source);
    debug_assert!(
        matches!(
            (&supplied, source.info().layer_kind),
            (Supplied::Tiles { kind: TileKind::Geometry, .. }, SourceKind::Geometry)
                | (Supplied::Tiles { kind: TileKind::Raster, .. } | Supplied::Image(_), SourceKind::Raster)
                | (Supplied::Tiles { kind: TileKind::Dem(_), .. }, SourceKind::RasterDem)
        ),
        "source type '{name}' supplies data of another kind than its layers draw"
    );
    Some(match supplied {
        Supplied::Tiles { kind, single_layer, origin } => {
            Box::new(RenderTileSource::new(source.clone(), kind, single_layer, origin, store))
        }
        Supplied::Image(image_of) => Box::new(RenderImageSource::new(source.clone(), image_of)),
    })
}
