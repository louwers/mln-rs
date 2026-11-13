// Port-only: how the members of the tile worker messages that serde can't
// derive cross (mod.rs): trait objects, as the types they can be, and the
// values that cross once per worker instance, then by key
// (mln_util::serialization::shared).

use std::sync::Arc;

use mln_style::layer_properties::LayerProperties;
use mln_style::serialization::shared_layer_properties;
use mln_tile_id::EncodedTileData;
use mln_util::serialization::shared::{self, Kind};
use serde::ser::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::bucket::{Bucket, BucketPtr};
use crate::buckets::circle_bucket::CircleBucket;
use crate::buckets::fill_bucket::FillBucket;
use crate::buckets::fill_extrusion_bucket::FillExtrusionBucket;
use crate::buckets::heatmap_bucket::HeatmapBucket;
use crate::buckets::line_bucket::LineBucket;
use crate::buckets::symbol_bucket::SymbolBucket;
use crate::geometry::feature_index::FeatureIndex;
use crate::gfx::dynamic_texture::DynamicTexture;
use crate::render_layer::LayerRenderData;
use crate::tile::geojson_tile_data::GeoJSONTileData;
use crate::tile::geometry_tile::{LayoutAtlases, LayoutResult};
use crate::tile::geometry_tile_worker::SharedTileData;

/// The `shared::Kind` of tile data: what it is made from
/// (`GeometryTileData::encoded`), which the other side makes it from again.
/// A layout's feature index refers to the data it indexes by key.
pub(crate) struct SharedTileDataKind;

impl Kind for SharedTileDataKind {
    type Value = dyn mln_tile_id::GeometryTileData + Send + Sync;
    const NAME: &'static str = "tile data";

    fn write<S: Serializer>(data: &Self::Value, serializer: S) -> Result<S::Ok, S::Error> {
        data.encoded()
            .ok_or_else(|| S::Error::custom("tile data that no longer has its bytes can't cross"))?
            .serialize(serializer)
    }

    fn read<'de, D: Deserializer<'de>>(deserializer: D) -> Result<SharedTileData, D::Error> {
        Ok(match EncodedTileData::deserialize(deserializer)? {
            EncodedTileData::Mvt(bytes) => Arc::new(mln_tile::VectorMVTTileData::new(bytes)),
            EncodedTileData::Mlt { data, fast_pfor_enabled } => {
                Arc::new(mln_tile::VectorMLTTileData::new(data, fast_pfor_enabled))
            }
            EncodedTileData::GeoJson(features) => Arc::new(GeoJSONTileData::new(Arc::unwrap_or_clone(features))),
        })
    }
}

/// `#[serde(with)]` for an `Option<SharedTileData>`.
pub(crate) mod tile_data {
    use super::{Deserializer, Serializer, SharedTileData, SharedTileDataKind, shared};

    #[expect(clippy::ref_option, reason = "`#[serde(with)]` passes the field by reference")]
    pub(crate) fn serialize<S: Serializer>(data: &Option<SharedTileData>, serializer: S) -> Result<S::Ok, S::Error> {
        shared::serialize_option::<SharedTileDataKind, S>(data, serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<SharedTileData>, D::Error> {
        shared::deserialize_option::<SharedTileDataKind, D>(deserializer)
    }
}

/// The `shared::Kind` of the textures of a worker instance's atlas, which
/// come from the instance: the map side keeps a copy of each
/// (`map_side.rs`, `AtlasMirror`), made from its size and pixel type.
pub(crate) struct SharedTexture;

impl Kind for SharedTexture {
    type Value = DynamicTexture;
    const NAME: &'static str = "texture";

    fn write<S: Serializer>(texture: &DynamicTexture, serializer: S) -> Result<S::Ok, S::Error> {
        (texture.get_size(), texture.get_pixel_format()).serialize(serializer)
    }

    fn read<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Arc<DynamicTexture>, D::Error> {
        let (size, pixel_type) = Deserialize::deserialize(deserializer)?;
        Ok(Arc::new(DynamicTexture::new(size, pixel_type)))
    }
}

/// `#[serde(with)]` for an `Option<DynamicTexturePtr>`.
pub(crate) mod texture {
    use super::{Arc, Deserializer, DynamicTexture, Serializer, SharedTexture, shared};

    #[expect(clippy::ref_option, reason = "`#[serde(with)]` passes the field by reference")]
    pub(crate) fn serialize<S: Serializer>(
        texture: &Option<Arc<DynamicTexture>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        shared::serialize_option::<SharedTexture, S>(texture, serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Arc<DynamicTexture>>, D::Error> {
        shared::deserialize_option::<SharedTexture, D>(deserializer)
    }
}

/// `#[serde(with)]` for the layers of a `setLayers`.
pub(crate) mod layers {
    use super::{Arc, Deserialize, Deserializer, LayerProperties, Serialize, Serializer, shared_layer_properties};

    #[derive(Serialize, Deserialize)]
    struct Layer(#[serde(with = "shared_layer_properties")] Arc<dyn LayerProperties>);

    pub(crate) fn serialize<S: Serializer>(
        layers: &[Arc<dyn LayerProperties>],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        layers.iter().cloned().map(Layer).collect::<Vec<_>>().serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<Arc<dyn LayerProperties>>, D::Error> {
        Ok(Vec::<Layer>::deserialize(deserializer)?.into_iter().map(|layer| layer.0).collect())
    }
}

/// The bucket types a `GeometryTileWorker` makes (debug, raster and
/// hillshade buckets never come from one), as a tagged enum: a bucket is a
/// trait object.
macro_rules! bucket_types {
    ($($tag:ident: $bucket:ty),+ $(,)?) => {
        #[derive(Serialize)]
        enum BucketRef<'a> { $($tag(&'a $bucket),)+ }

        #[expect(clippy::large_enum_variant, reason = "the value is read, then moved into an `Arc` at once")]
        #[derive(Deserialize)]
        enum BucketOwned { $($tag($bucket),)+ }

        fn bucket_ref(bucket: &dyn Bucket) -> Option<BucketRef<'_>> {
            $(if let Some(bucket) = bucket.downcast_ref::<$bucket>() {
                return Some(BucketRef::$tag(bucket));
            })+
            None
        }

        fn bucket_ptr(bucket: BucketOwned) -> BucketPtr {
            match bucket { $(BucketOwned::$tag(bucket) => Arc::new(bucket),)+ }
        }
    };
}

bucket_types! {
    Fill: FillBucket,
    Line: LineBucket,
    Circle: CircleBucket,
    Heatmap: HeatmapBucket,
    FillExtrusion: FillExtrusionBucket,
    Symbol: SymbolBucket,
}

/// The layers that render a bucket, with the properties they were laid out
/// with, which cross back by key: the map side gets the ones it sent.
#[derive(Serialize, Deserialize)]
struct RenderingLayer(String, #[serde(with = "shared_layer_properties")] Arc<dyn LayerProperties>);

/// A `LayoutResult` as serde writes it: each bucket once, with the layers
/// that render it (a layer group's layers share their bucket), and the
/// feature index. The atlases cross apart from the layout (envelope.rs), so
/// that the map side releases their bins even when it can't read the rest;
/// a layout read back has empty ones until the codec sets them.
#[derive(Serialize)]
struct LayoutRef<'a> {
    buckets: Vec<(BucketRef<'a>, Vec<RenderingLayer>)>,
    feature_index: &'a FeatureIndex,
}

#[derive(Deserialize)]
struct LayoutOwned {
    buckets: Vec<(BucketOwned, Vec<RenderingLayer>)>,
    feature_index: FeatureIndex,
}

impl LayoutResult {
    /// Each bucket with the layers that render it, in layer id order.
    fn buckets(&self) -> Result<Vec<(BucketRef<'_>, Vec<RenderingLayer>)>, &'static str> {
        let mut layer_ids: Vec<&String> = self.layer_render_data.keys().collect();
        layer_ids.sort();
        let mut buckets: Vec<(&BucketPtr, Vec<RenderingLayer>)> = Vec::new();
        for layer_id in layer_ids {
            let LayerRenderData { bucket, layer_properties } = &self.layer_render_data[layer_id];
            let layer = RenderingLayer(layer_id.clone(), Arc::clone(layer_properties));
            match buckets.iter_mut().find(|(shared, _)| Arc::ptr_eq(shared, bucket)) {
                Some((_, layers)) => layers.push(layer),
                None => buckets.push((bucket, vec![layer])),
            }
        }
        buckets
            .into_iter()
            .map(|(bucket, layers)| {
                bucket_ref(bucket.as_ref())
                    .map(|bucket| (bucket, layers))
                    .ok_or("a bucket type that no tile worker makes")
            })
            .collect()
    }
}

/// The encoding of a layout's buckets with the layers that render them, as
/// `LayoutResult` writes them outside a codec's session.
#[cfg(test)]
pub(super) fn encode_buckets(result: &LayoutResult) -> Vec<u8> {
    postcard::to_allocvec(&result.buckets().unwrap()).unwrap()
}

impl Serialize for LayoutResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let buckets = self.buckets().map_err(S::Error::custom)?;
        LayoutRef { buckets, feature_index: &self.feature_index }.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for LayoutResult {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let layout = LayoutOwned::deserialize(deserializer)?;
        let mut layer_render_data = std::collections::HashMap::new();
        for (bucket, layers) in layout.buckets {
            let bucket = bucket_ptr(bucket);
            for RenderingLayer(layer_id, layer_properties) in layers {
                layer_render_data.insert(layer_id, LayerRenderData { bucket: Arc::clone(&bucket), layer_properties });
            }
        }
        Ok(LayoutResult {
            layer_render_data,
            feature_index: Arc::new(layout.feature_index),
            atlases: Arc::new(LayoutAtlases::default()),
        })
    }
}
