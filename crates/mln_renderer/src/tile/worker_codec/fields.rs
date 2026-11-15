// Port-only: how the members of the tile workers' messages that the derive
// can't write cross (mod.rs): trait objects, as the types they can be, and
// the values that cross once per worker instance, then by key
// (mln_util::serialization::shared).

use std::sync::Arc;

use mln_style::layer_properties::LayerProperties;
use mln_style::serialization::shared_layer_properties;
use mln_util::serialization::shared;
use mln_vec_map::VecSet;
use mln_wire::{Reader, Wire, Writer};

use crate::bucket::{Bucket, BucketPtr};
use crate::buckets::circle_bucket::CircleBucket;
use crate::buckets::fill_bucket::FillBucket;
use crate::buckets::fill_extrusion_bucket::FillExtrusionBucket;
use crate::buckets::heatmap_bucket::HeatmapBucket;
use crate::buckets::line_bucket::LineBucket;
use crate::buckets::symbol_bucket::SymbolBucket;
use crate::geometry::feature_index::FeatureIndex;
use crate::render_layer::LayerRenderData;
use crate::tile::geometry_tile::LayoutResult;
use crate::tile::job::SourceStyle;

/// `#[wire(with)]` for a source style's layers, each by key.
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

/// The `shared::Kind` of a source's style: its version, its layers by key,
/// its global state and font faces.
pub(crate) struct SharedSourceStyle;

impl shared::Kind for SharedSourceStyle {
    type Value = SourceStyle;
    const NAME: &'static str = "source style";
}

impl shared::WireKind for SharedSourceStyle {
    fn write(style: &SourceStyle, w: &mut Writer) {
        style.version.write(w);
        layers::write(&style.layers, w);
        style.global_state.write(w);
        style.font_faces.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<SourceStyle>> {
        Some(Arc::new(SourceStyle {
            version: u64::read(r)?,
            layers: layers::read(r)?,
            global_state: Wire::read(r)?,
            font_faces: Wire::read(r)?,
        }))
    }
}

/// `#[wire(with)]` for a job's source style.
pub(crate) mod source_style {
    use super::{Arc, Reader, SharedSourceStyle, SourceStyle, Writer, shared};

    pub(crate) fn write(style: &Arc<SourceStyle>, w: &mut Writer) {
        shared::write::<SharedSourceStyle>(style, w);
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Option<Arc<SourceStyle>> {
        shared::read::<SharedSourceStyle>(r)
    }
}

/// The `shared::Kind` of the sets of images that exist.
pub(crate) struct SharedAvailableImages;

impl shared::Kind for SharedAvailableImages {
    type Value = VecSet<String>;
    const NAME: &'static str = "available images";
}

impl shared::WireKind for SharedAvailableImages {
    fn write(images: &VecSet<String>, w: &mut Writer) {
        images.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<VecSet<String>>> {
        VecSet::read(r).map(Arc::new)
    }
}

/// `#[wire(with)]` for a job's available images.
pub(crate) mod available_images {
    use super::{Arc, Reader, SharedAvailableImages, VecSet, Writer, shared};

    pub(crate) fn write(images: &Arc<VecSet<String>>, w: &mut Writer) {
        shared::write::<SharedAvailableImages>(images, w);
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Option<Arc<VecSet<String>>> {
        shared::read::<SharedAvailableImages>(r)
    }
}

/// The bucket types a layout makes (debug, raster and
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
/// it (a layer group's layers share their bucket), and the feature index
/// without its data.
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
        Some(LayoutResult { layer_render_data, feature_index: Arc::new(FeatureIndex::read(r)?) })
    }
}
