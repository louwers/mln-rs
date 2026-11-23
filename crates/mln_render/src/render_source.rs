// A render source is the renderer's side of a style source: a view into the
// renderer's tile store, which keeps a source's tiles, or an image source's
// image.

use std::sync::Arc;

use mln_map_types::MapDebugOptions;
use mln_style::Source;

use crate::erased_layer::ErasedEvaluatedLayer;
use crate::gfx::context::Resources;
use crate::paint_parameters::TransformParameters;
use crate::registry::Registry;
use crate::render_tile::RenderTile;
use crate::source::Supplied;
use crate::sources::render_image_source::RenderImageSource;
use crate::sources::render_tile_source::RenderTileSource;
use crate::tile::job::SourceKey;
use crate::tile::raster::RasterContents;
use crate::tile::store::TileStore;
use crate::tile_parameters::TileParameters;

pub(crate) struct RenderTileParameters<'a> {
    pub(crate) transform: &'a TransformParameters,
    pub(crate) debug_options: MapDebugOptions,
    pub(crate) source_name: &'a str,
}

/// The render source of a style source, by what its type supplies.
pub(crate) enum RenderSource {
    Tiles(RenderTileSource),
    Image(Box<RenderImageSource>),
}

impl RenderSource {
    /// The render source of a style source through its type in `registry`,
    /// whose tiles, if it has any, `store` keeps. `None` for a type that the
    /// registry lacks, which breaks the contract that a map and its
    /// renderer register the same types.
    pub(crate) fn new(registry: &Registry, source: &Source, store: &mut TileStore) -> Option<Self> {
        let name = source.info().name;
        let Some(source_type) = registry.source_type(name) else {
            debug_assert!(false, "the renderer's registry has no source type '{name}'");
            return None;
        };
        Some(match source_type.supply(source) {
            Supplied::Tiles(origin) => RenderSource::Tiles(RenderTileSource::new(source.id(), origin, store)),
            Supplied::Image(image_of) => {
                RenderSource::Image(Box::new(RenderImageSource::new(source.clone(), image_of)))
            }
        })
    }

    pub(crate) fn is_loaded(&self, store: &TileStore) -> bool {
        match self {
            RenderSource::Tiles(source) => source.is_loaded(store),
            RenderSource::Image(source) => source.is_loaded(),
        }
    }

    /// Brings the source to `style_source`, the source's version of the
    /// frame, for `layers`, the frame's layers of it.
    pub(crate) fn update(
        &mut self,
        store: &mut TileStore,
        style_source: &Source,
        layers: &[Arc<dyn ErasedEvaluatedLayer>],
        needs_rendering: bool,
        needs_relayout: bool,
        parameters: &TileParameters<'_>,
    ) {
        match self {
            RenderSource::Tiles(source) => {
                source.update(store, style_source, layers, needs_rendering, needs_relayout, parameters);
            }
            RenderSource::Image(source) => source.update(style_source, needs_rendering, parameters),
        }
    }

    /// Readies what the frame draws of an enabled source, before its layers
    /// make their draws, uploading through `context`.
    pub(crate) fn upload(&mut self, store: &mut TileStore, context: &mut Resources) {
        match self {
            RenderSource::Tiles(source) => source.upload(store, context),
            RenderSource::Image(source) => source.upload(context),
        }
    }

    /// The source's tiles in the tile store, for a source with tiles.
    pub(crate) fn tiles(&self) -> Option<SourceKey> {
        match self {
            RenderSource::Tiles(source) => Some(source.key),
            RenderSource::Image(_) => None,
        }
    }

    /// The source's render tiles of the frame.
    pub(crate) fn render_tiles(&self) -> &[RenderTile] {
        match self {
            RenderSource::Tiles(source) => &source.render_tiles,
            RenderSource::Image(source) => source.render_tiles(),
        }
    }

    /// The source's tiles that its layers draw, by tile id and without the
    /// tiles held for fade, as indexes into
    /// [`render_tiles`](Self::render_tiles).
    pub(crate) fn tile_order(&self) -> &[usize] {
        match self {
            RenderSource::Tiles(source) => &source.order_by_id,
            RenderSource::Image(source) => source.tile_order(),
        }
    }

    /// An image source's image, which each of its render tiles shows;
    /// `None` for other sources and before the image arrives.
    pub(crate) fn image(&self) -> Option<&RasterContents> {
        match self {
            RenderSource::Tiles(_) => None,
            RenderSource::Image(source) => source.image.as_ref(),
        }
    }

    /// The deepest zoom that has tiles of its own, as the source's last
    /// [`Cover`](crate::source::Cover) says; `None` for a source without
    /// tiles or before its first cover.
    pub(crate) fn max_zoom(&self) -> Option<u8> {
        match self {
            RenderSource::Tiles(source) => source.max_zoom,
            RenderSource::Image(_) => None,
        }
    }
}
