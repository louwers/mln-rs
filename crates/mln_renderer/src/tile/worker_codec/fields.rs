// Port-only: how the members of the tile worker messages that the derive
// can't write cross (mod.rs): trait objects, as the types they can be, and
// the values that cross once per worker instance, then by key
// (mln_util::serialization::shared).

use std::sync::Arc;

use mln_style::layer_properties::LayerProperties;
use mln_style::serialization::shared_layer_properties;
use mln_util::serialization::shared::{self, Kind, WireKind};
use mln_wire::{Reader, Wire, Writer};

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
use crate::tile::geometry_tile::{LayoutAtlases, LayoutResult};

/// The `shared::Kind` of the textures of a worker instance's atlas, which
/// come from the instance: the map side keeps a copy of each
/// (`map_side.rs`, `AtlasMirror`), made from its size and pixel type.
pub(crate) struct SharedTexture;

impl Kind for SharedTexture {
    type Value = DynamicTexture;
    const NAME: &'static str = "texture";
}

impl WireKind for SharedTexture {
    fn write(texture: &DynamicTexture, w: &mut Writer) {
        texture.get_size().write(w);
        texture.get_pixel_format().write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<DynamicTexture>> {
        let size = Wire::read(r)?;
        let pixel_type = Wire::read(r)?;
        Some(Arc::new(DynamicTexture::new(size, pixel_type)))
    }
}

/// `#[wire(with)]` for an `Option<DynamicTexturePtr>`.
pub(crate) mod texture {
    use super::{Arc, DynamicTexture, Reader, SharedTexture, Writer, shared};

    #[expect(clippy::ref_option, reason = "`#[wire(with)]` passes the field by reference")]
    pub(crate) fn write(texture: &Option<Arc<DynamicTexture>>, w: &mut Writer) {
        shared::write_option::<SharedTexture>(texture, w);
    }

    #[expect(clippy::option_option, reason = "a read that can fail, of an `Option`")]
    pub(crate) fn read(r: &mut Reader<'_>) -> Option<Option<Arc<DynamicTexture>>> {
        shared::read_option::<SharedTexture>(r)
    }
}

/// `#[wire(with)]` for a `DynamicTexturePtr`.
pub(crate) mod shared_texture {
    use super::{Arc, DynamicTexture, Reader, SharedTexture, Writer, shared};

    pub(crate) fn write(texture: &Arc<DynamicTexture>, w: &mut Writer) {
        shared::write::<SharedTexture>(texture, w);
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Option<Arc<DynamicTexture>> {
        shared::read::<SharedTexture>(r)
    }
}

/// `#[wire(with)]` for the layers of a `setLayers`.
pub(crate) mod layers {
    use super::{Arc, LayerProperties, Reader, Writer, shared_layer_properties};

    pub(crate) fn write(layers: &[Arc<dyn LayerProperties>], w: &mut Writer) {
        mln_wire::Wire::write(&layers.len(), w);
        for layer in layers {
            shared_layer_properties::write(layer, w);
        }
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Option<Vec<Arc<dyn LayerProperties>>> {
        let len = r.length()?;
        let mut layers = Vec::with_capacity(len.min(r.rest().len()));
        for _ in 0..len {
            layers.push(shared_layer_properties::read(r)?);
        }
        Some(layers)
    }
}

/// The bucket types a `GeometryTileWorker` makes (debug, raster and
/// hillshade buckets never come from one), each as its index and then the
/// bucket: a bucket is a trait object.
macro_rules! bucket_types {
    ($($bucket:ty),+ $(,)?) => {
        fn write_bucket(bucket: &dyn Bucket, w: &mut Writer) {
            let mut index = 0u32;
            $(if let Some(bucket) = bucket.downcast_ref::<$bucket>() {
                mln_wire::write_tag(w, index);
                return bucket.write(w);
            }
            index += 1;)+
            let _ = index;
            w.fail("a bucket type that no tile worker makes".to_owned());
        }

        fn read_bucket(r: &mut Reader<'_>) -> Option<BucketPtr> {
            let tag = r.tag()?;
            let mut index = 0u32;
            $(if tag == index {
                return Some(Arc::new(<$bucket>::read(r)?));
            }
            index += 1;)+
            let _ = index;
            r.fail_tag()
        }
    };
}

bucket_types! {
    FillBucket,
    LineBucket,
    CircleBucket,
    HeatmapBucket,
    FillExtrusionBucket,
    SymbolBucket,
}

/// A bucket, and the ids of the layers that render it with the properties
/// they were laid out with.
type RenderedBucket<'a> = (&'a BucketPtr, Vec<(&'a String, &'a Arc<dyn LayerProperties>)>);

impl LayoutResult {
    /// Each bucket with the layers that render it, in layer id order.
    fn buckets(&self) -> Vec<RenderedBucket<'_>> {
        let mut layer_ids: Vec<&String> = self.layer_render_data.keys().collect();
        layer_ids.sort_unstable();
        let mut buckets: Vec<RenderedBucket<'_>> = Vec::new();
        for layer_id in layer_ids {
            let LayerRenderData { bucket, layer_properties } = &self.layer_render_data[layer_id];
            match buckets.iter_mut().find(|(shared, _)| Arc::ptr_eq(shared, bucket)) {
                Some((_, layers)) => layers.push((layer_id, layer_properties)),
                None => buckets.push((bucket, vec![(layer_id, layer_properties)])),
            }
        }
        buckets
    }

    fn write_buckets(&self, w: &mut Writer) {
        let buckets = self.buckets();
        buckets.len().write(w);
        for (bucket, layers) in buckets {
            write_bucket(bucket.as_ref(), w);
            layers.len().write(w);
            for (layer_id, properties) in layers {
                layer_id.write(w);
                // The layers that render a bucket cross back by key: the map
                // side gets the properties it sent.
                shared_layer_properties::write(properties, w);
            }
        }
    }
}

/// The encoding of a layout's buckets with the layers that render them, as
/// `LayoutResult` writes them outside a codec's session.
#[cfg(test)]
pub(super) fn encode_buckets(result: &LayoutResult) -> Vec<u8> {
    let mut w = Writer::new();
    result.write_buckets(&mut w);
    w.finish().unwrap()
}

/// A `LayoutResult` crosses as each bucket once, with the layers that render
/// it (a layer group's layers share their bucket), and the feature index.
/// The atlases cross apart from the layout (envelope.rs), so that the map
/// side releases their bins even when it can't read the rest; a layout read
/// back has empty ones until the codec sets them.
impl Wire for LayoutResult {
    fn write(&self, w: &mut Writer) {
        self.write_buckets(w);
        self.feature_index.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Self> {
        let mut layer_render_data = mln_util::hash::HashMap::default();
        let buckets = r.length()?;
        for _ in 0..buckets {
            let bucket = read_bucket(r)?;
            let layers = r.length()?;
            for _ in 0..layers {
                let layer_id = String::read(r)?;
                let layer_properties = shared_layer_properties::read(r)?;
                layer_render_data.insert(layer_id, LayerRenderData { bucket: Arc::clone(&bucket), layer_properties });
            }
        }
        Some(LayoutResult {
            layer_render_data,
            feature_index: Arc::new(FeatureIndex::read(r)?),
            atlases: Arc::new(LayoutAtlases::default()),
        })
    }
}
