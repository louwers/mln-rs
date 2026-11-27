//! What a layer type draws from: its [`TileInput`], a kind of tile and what
//! such a tile holds for a layer of the type, its [`Data`](TileInput::Data).
//! The kinds are the engine's: [`Features`] of a geometry source, which a
//! layout turns into a bucket per tile, the images of [`RasterTiles`] (a
//! raster source's tiles, or the one image of an image source), the
//! elevations of [`DemTiles`], and [`NoTiles`] for a layer without a source.
//! The registry checks that a type's input is of the kind of source its
//! style half names.

use std::marker::PhantomData;
use std::sync::Arc;

use mln_style::bitmap::PremultipliedImage;
use mln_style::layer::SourceKind;

use crate::draw::identity::SimpleIdentity;
use crate::draw::tile_render_data::TileAtlasTextures;
use crate::layer::bucket::Bucket;
use crate::layer::bucket_geometry::BucketGeometry;
use crate::layer::layer_type::{EvaluatedLayer, LayerType};
use crate::layer::layout::Layout;
use crate::query::feature_index::FeatureIndex;
use crate::render::tile::{TileContent, ViewPlace};
use crate::source::render_image_source::{ImageQuad, ImageTile};
use crate::source::{DemTiles, RasterTiles};
use crate::text::atlas::ImagePosition;
use crate::tile_store::contents::GeometryContents;
use crate::tile_store::dem::DemContents;
use crate::tile_store::raster::RasterContents;

/// What a layer of the type `T` draws from: a kind of tile and what such a
/// tile holds for the layer, its [`Data`](Self::Data).
pub(crate) trait TileInput<T: LayerType>: input::Find<T> + 'static {
    /// What a frame's tile holds for the layer.
    type Data<'a>: Copy;
}

/// What a frame's tile holds for a layer of the type `T`: its
/// [`LayerType::Input`]'s [`Data`](TileInput::Data).
pub(crate) type TileData<'a, T> = <<T as LayerType>::Input as TileInput<T>>::Data<'a>;

/// The tiles of a geometry source, whose features the layout `L` turns into
/// a bucket per tile. A tile's data is its [`LayerBucket`].
pub(crate) struct Features<L>(PhantomData<fn() -> L>);

impl<T: LayerType, L: Layout<T>> TileInput<T> for Features<L> {
    type Data<'a> = LayerBucket<'a, T, L::Bucket>;
}

/// No tiles, for a layer without a source, which picks what it draws
/// itself.
pub(crate) enum NoTiles {}

impl<T: LayerType> TileInput<T> for NoTiles {
    type Data<'a> = std::convert::Infallible;
}

/// The tiles of a raster source, or of an image source, which draws its one
/// image in the tiles it covers. A tile's data is its [`RasterTile`].
impl<T: LayerType> TileInput<T> for RasterTiles {
    type Data<'a> = RasterTile<'a>;
}

/// The tiles of a raster-dem source. A tile's data is its DEM.
impl<T: LayerType> TileInput<T> for DemTiles {
    type Data<'a> = &'a DemContents;
}

/// A bucket of a layer of the type `T` in a tile's layout, of the bucket
/// type `B`, with the layer's values it was laid out with and the rest of
/// what the layout's contents hold for the tile's draws.
pub(crate) struct LayerBucket<'a, T: LayerType, B> {
    pub(crate) bucket: &'a B,
    /// The bucket's identity, which tells apart what layers made of it.
    pub(crate) id: SimpleIdentity,
    /// The index of the bucket among the layout's buckets.
    pub(crate) bucket_index: usize,
    /// The values of the layer that the bucket was laid out with.
    pub(crate) layer: &'a EvaluatedLayer<T>,
    contents: &'a GeometryContents,
}

impl<T: LayerType, B> Clone for LayerBucket<'_, T, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: LayerType, B> Copy for LayerBucket<'_, T, B> {}

impl<'a, T: LayerType, B: Bucket> LayerBucket<'a, T, B> {
    /// The bucket of the layer at `index` among those `contents` laid out,
    /// if the layout laid it out as a layer of the type `T` into a `B`.
    pub(crate) fn laid(contents: &'a GeometryContents, index: usize) -> Option<Self> {
        let laid = contents.result.layer_render_data.laid(index)?;
        let layer: &dyn std::any::Any = laid.layer;
        Some(LayerBucket {
            bucket: laid.bucket.downcast_ref::<B>()?,
            id: laid.bucket.id(),
            bucket_index: laid.bucket_index,
            layer: layer.downcast_ref()?,
            contents,
        })
    }
}

impl<'a, T: LayerType, B> LayerBucket<'a, T, B> {
    /// The geometry of the layout's buckets in the context's stores, which
    /// the draws bind.
    #[must_use]
    pub(crate) fn geometry(self) -> &'a BucketGeometry {
        &self.contents.bucket_geometry
    }

    /// The feature index of the layout.
    #[must_use]
    pub(crate) fn feature_index(self) -> &'a Arc<FeatureIndex> {
        &self.contents.result.feature_index
    }

    /// Where the pattern `pattern` is in the tile's icon atlas.
    #[must_use]
    pub(crate) fn pattern(self, pattern: &str) -> Option<ImagePosition> {
        self.contents.pattern_positions()?.get(pattern).cloned()
    }

    /// The textures of the tile's glyph and icon atlases.
    #[must_use]
    pub(crate) fn atlas_textures(self) -> &'a TileAtlasTextures {
        &self.contents.textures
    }
}

/// What a raster tile, or a tile that an image source's image draws in,
/// holds for a layer: an image, and the parts of the tile it draws.
#[derive(Clone, Copy)]
pub(crate) enum RasterTile<'a> {
    /// A raster tile's image, drawn on the parts of the tile that its mask
    /// keeps.
    Tile(&'a RasterContents),
    /// An image source's image, drawn on its mesh, which the image's quad
    /// puts in the tile.
    Image(&'a ImageTile),
}

impl<'a> RasterTile<'a> {
    /// The identity of the image, by which a layer keeps its texture.
    #[must_use]
    pub(crate) fn id(self) -> SimpleIdentity {
        match self {
            RasterTile::Tile(raster) => raster.id(),
            RasterTile::Image(image) => image.id(),
        }
    }

    #[must_use]
    pub(crate) fn image(self) -> &'a PremultipliedImage {
        match self {
            RasterTile::Tile(raster) => &raster.image,
            RasterTile::Image(image) => &image.image,
        }
    }

    /// The number of times the parts of the tile it draws changed: its
    /// mask's changes, or its mesh's.
    #[must_use]
    pub(crate) fn generation(self) -> u64 {
        match self {
            RasterTile::Tile(raster) => raster.masking.generation(),
            RasterTile::Image(image) => image.generation(),
        }
    }

    /// Where a draw puts the vertices of the tile's geometry: on the tile
    /// itself, or on an image source's quad. Every tile of a source draws on
    /// the same quad.
    #[must_use]
    pub(crate) fn quad(self) -> ImageQuad {
        match self {
            RasterTile::Tile(_) => ImageQuad::TILE,
            RasterTile::Image(image) => image.quad,
        }
    }
}

pub(crate) mod input {
    use std::sync::Arc;

    use mln_style::error::Error;
    use mln_wire::Reader;

    use super::{
        DemTiles, Features, LayerBucket, NoTiles, RasterTile, RasterTiles, SourceKind, TileContent, TileInput,
        ViewPlace,
    };
    use crate::layer::bucket::{ErasedBucket, Stored};
    use crate::layer::layer_type::{self, ErasedEvaluatedLayer, ErasedLayout, LayerType};
    use crate::layer::layout::{Layout, LayoutParameters, SourceLayer};
    use crate::render::pass::RenderPass;
    use crate::source::GeometryTiles;
    use crate::source::kind::Deliver;
    use crate::tile_store::contents::{Contents, GeometryContents};

    /// The engine's side of a [`TileInput`]: the kind of source whose tiles
    /// hold it, the layout of a tile's features for an input that lays
    /// out, and what a frame's tile holds for a layer of it. It seals
    /// [`TileInput`].
    pub(crate) trait Find<T: LayerType> {
        /// The kind of source whose tiles hold the input.
        const KIND: SourceKind;

        /// [`Layout::needs_resources`] of `group`, for an input that lays
        /// out; false otherwise.
        fn needs_resources(_group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool {
            false
        }

        /// The layout of `group`, layers of the same layout on
        /// `source_layer`, with the dependencies it records in
        /// `parameters`; `None` for an input that doesn't lay out. Fails for
        /// a layer of another type.
        fn create_layout(
            _group: &[Arc<dyn ErasedEvaluatedLayer>],
            _parameters: LayoutParameters<'_>,
            _source_layer: SourceLayer,
        ) -> Result<Option<Box<dyn ErasedLayout>>, Error> {
            Ok(None)
        }

        /// Reads a bucket of the input's layout.
        fn read_bucket(r: &mut Reader<'_>) -> Option<Box<dyn ErasedBucket>> {
            r.fail("a bucket of a layer type that doesn't lay out")
        }

        /// What `contents`, a tile's layout, holds for the layer `layer_id`,
        /// for an input that lays out.
        fn laid<'a>(_contents: &'a GeometryContents, _layer_id: &str) -> Option<<Self as TileInput<T>>::Data<'a>>
        where
            Self: TileInput<T>,
        {
            None
        }

        /// What `content` holds for the layer `layer_id`, if anything, and
        /// where it is in it.
        fn find<'a>(content: TileContent<'a>, layer_id: &str) -> Option<(<Self as TileInput<T>>::Data<'a>, ViewPlace)>
        where
            Self: TileInput<T>;

        /// What `content` holds at `place`, where [`find`](Self::find)
        /// found what it holds for a layer in the same content.
        fn at(content: TileContent<'_>, place: ViewPlace) -> Option<<Self as TileInput<T>>::Data<'_>>
        where
            Self: TileInput<T>;

        /// Whether `data` is what a tile holds for the layer `layer_id`,
        /// rather than for another layer.
        fn is_for(_data: <Self as TileInput<T>>::Data<'_>, _layer_id: &str) -> bool
        where
            Self: TileInput<T>,
        {
            true
        }

        /// The passes that `data` was laid out to draw in, which its layer
        /// draws in too; none for data that isn't laid out.
        fn passes(_data: <Self as TileInput<T>>::Data<'_>) -> RenderPass
        where
            Self: TileInput<T>,
        {
            RenderPass::None
        }
    }

    impl<T: LayerType, L: Layout<T>> Find<T> for Features<L> {
        const KIND: SourceKind = <GeometryTiles as Deliver>::KIND;

        fn needs_resources(group: &[Arc<dyn ErasedEvaluatedLayer>]) -> bool {
            layer_type::needs_resources::<T, L>(group)
        }

        fn create_layout(
            group: &[Arc<dyn ErasedEvaluatedLayer>],
            parameters: LayoutParameters<'_>,
            source_layer: SourceLayer,
        ) -> Result<Option<Box<dyn ErasedLayout>>, Error> {
            layer_type::create_layout::<T, L>(group, parameters, source_layer).map(Some)
        }

        fn read_bucket(r: &mut Reader<'_>) -> Option<Box<dyn ErasedBucket>> {
            Some(Box::new(Stored::new(<L::Bucket as mln_wire::Wire>::read(r)?)))
        }

        fn laid<'a>(contents: &'a GeometryContents, layer_id: &str) -> Option<<Self as TileInput<T>>::Data<'a>> {
            LayerBucket::laid(contents, contents.result.layer_render_data.find(layer_id)?)
        }

        fn find<'a>(content: TileContent<'a>, layer_id: &str) -> Option<(<Self as TileInput<T>>::Data<'a>, ViewPlace)> {
            let TileContent::Tile(contents) = content else { return None };
            let contents = contents.geometry()?;
            let index = contents.result.layer_render_data.find(layer_id)?;
            Some((LayerBucket::laid(contents, index)?, ViewPlace::Laid(index)))
        }

        fn at(content: TileContent<'_>, place: ViewPlace) -> Option<<Self as TileInput<T>>::Data<'_>> {
            let ViewPlace::Laid(index) = place else {
                debug_assert!(false, "a tile's own contents' place for a laid-out layer");
                return None;
            };
            let TileContent::Tile(contents) = content else { return None };
            LayerBucket::laid(contents.geometry()?, index)
        }

        fn is_for(data: <Self as TileInput<T>>::Data<'_>, layer_id: &str) -> bool {
            data.layer.style.id == layer_id
        }

        fn passes(data: <Self as TileInput<T>>::Data<'_>) -> RenderPass {
            RenderPass(data.layer.render_passes)
        }
    }

    impl<T: LayerType> Find<T> for RasterTiles {
        const KIND: SourceKind = <RasterTiles as Deliver>::KIND;

        fn find<'a>(
            content: TileContent<'a>,
            _layer_id: &str,
        ) -> Option<(<Self as TileInput<T>>::Data<'a>, ViewPlace)> {
            Some((<Self as Find<T>>::at(content, ViewPlace::Own)?, ViewPlace::Own))
        }

        fn at(content: TileContent<'_>, place: ViewPlace) -> Option<<Self as TileInput<T>>::Data<'_>> {
            debug_assert_eq!(place, ViewPlace::Own, "a laid-out layer's place for a tile's own contents");
            match content {
                TileContent::Tile(Contents::Raster(raster)) => Some(RasterTile::Tile(raster)),
                TileContent::Image(image) => Some(RasterTile::Image(image)),
                TileContent::Tile(_) => None,
            }
        }
    }

    impl<T: LayerType> Find<T> for DemTiles {
        const KIND: SourceKind = <DemTiles as Deliver>::KIND;

        fn find<'a>(
            content: TileContent<'a>,
            _layer_id: &str,
        ) -> Option<(<Self as TileInput<T>>::Data<'a>, ViewPlace)> {
            Some((<Self as Find<T>>::at(content, ViewPlace::Own)?, ViewPlace::Own))
        }

        fn at(content: TileContent<'_>, place: ViewPlace) -> Option<<Self as TileInput<T>>::Data<'_>> {
            debug_assert_eq!(place, ViewPlace::Own, "a laid-out layer's place for a tile's own contents");
            match content {
                TileContent::Tile(Contents::Dem(dem)) => Some(dem),
                _ => None,
            }
        }
    }

    impl<T: LayerType> Find<T> for NoTiles {
        const KIND: SourceKind = SourceKind::None;

        fn find<'a>(
            _content: TileContent<'a>,
            _layer_id: &str,
        ) -> Option<(<Self as TileInput<T>>::Data<'a>, ViewPlace)> {
            None
        }

        fn at(_content: TileContent<'_>, _place: ViewPlace) -> Option<<Self as TileInput<T>>::Data<'_>> {
            None
        }
    }
}
