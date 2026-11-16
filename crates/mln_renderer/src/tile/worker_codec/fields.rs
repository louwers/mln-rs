// Port-only: how the members of the tile workers' messages that the derive
// can't write cross (mod.rs): trait objects, as the types they can be, and
// the values that cross once per worker instance, then by key
// (mln_util::serialization::shared).

use std::sync::Arc;

use mln_util::serialization::shared;
use mln_vec_map::VecSet;
use mln_wire::{Reader, Wire, Writer};

use super::decoding_layer_type;
use crate::bucket::BucketPtr;
use crate::geometry::feature_index::FeatureIndex;
use crate::layer_type::ErasedEvaluatedLayer;
use crate::render_layer::LayerRenderData;
use crate::tile::contents::LayoutResult;
use crate::tile::job::SourceStyle;

/// The `shared::Kind` of layers' evaluated values: their type's name, then
/// the values, which the reading side reads as the type that its registry
/// has by the name.
pub(crate) struct SharedEvaluatedLayer;

impl shared::Kind for SharedEvaluatedLayer {
    type Value = dyn ErasedEvaluatedLayer;
    const NAME: &'static str = "evaluated layer";
}

impl shared::WireKind for SharedEvaluatedLayer {
    fn write(layer: &dyn ErasedEvaluatedLayer, w: &mut Writer) {
        mln_wire::write_str(w, layer.style().get_type_info().name);
        layer.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>> {
        let name = mln_wire::read_str(r)?;
        match decoding_layer_type(name) {
            Ok(layer_type) => layer_type.read_evaluated(r),
            Err(message) => r.fail(message),
        }
    }
}

/// `#[wire(with)]` for a layer's evaluated values, by key.
pub(crate) mod evaluated_layer {
    use super::{Arc, ErasedEvaluatedLayer, Reader, SharedEvaluatedLayer, Writer, shared};

    pub(crate) fn write(layer: &Arc<dyn ErasedEvaluatedLayer>, w: &mut Writer) {
        shared::write::<SharedEvaluatedLayer>(layer, w);
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>> {
        shared::read::<SharedEvaluatedLayer>(r)
    }
}

/// `#[wire(with)]` for a source style's layers, each by key.
pub(crate) mod layers {
    use super::{Arc, ErasedEvaluatedLayer, Reader, Writer, evaluated_layer};

    pub(crate) fn write(layers: &[Arc<dyn ErasedEvaluatedLayer>], w: &mut Writer) {
        mln_wire::Wire::write(&layers.len(), w);
        for layer in layers {
            evaluated_layer::write(layer, w);
        }
    }

    pub(crate) fn read(r: &mut Reader<'_>) -> Option<Vec<Arc<dyn ErasedEvaluatedLayer>>> {
        let len = r.length()?;
        let mut layers = Vec::with_capacity(len.min(r.rest().len()));
        for _ in 0..len {
            layers.push(evaluated_layer::read(r)?);
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

/// A bucket, and the ids of the layers that render it with the properties
/// they were laid out with.
type RenderedBucket<'a> = (&'a BucketPtr, Vec<(&'a String, &'a Arc<dyn ErasedEvaluatedLayer>)>);

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
            layers.len().write(w);
            for (layer_id, properties) in layers {
                layer_id.write(w);
                // The layers that render a bucket cross back by key: the map
                // side gets the properties it sent.
                evaluated_layer::write(properties, w);
            }
            // The reader reads the bucket as its layers' type's.
            bucket.write_wire(w);
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

/// A `LayoutResult` crosses as each bucket once, after the layers that
/// render it (a layer group's layers share their bucket, and their type is
/// the bucket's), and the feature index without its data.
impl Wire for LayoutResult {
    fn write(&self, w: &mut Writer) {
        self.write_buckets(w);
        self.feature_index.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Self> {
        let mut layer_render_data = mln_util::hash::HashMap::default();
        let buckets = r.length()?;
        for _ in 0..buckets {
            let len = r.length()?;
            let mut layers = Vec::with_capacity(len.min(r.rest().len()));
            for _ in 0..len {
                layers.push((String::read(r)?, evaluated_layer::read(r)?));
            }
            let Some((_, leader)) = layers.first() else {
                return r.fail("a bucket that no layer renders");
            };
            let bucket = leader.entry().read_bucket(r)?;
            for (layer_id, layer_properties) in layers {
                layer_render_data.insert(layer_id, LayerRenderData { bucket: Arc::clone(&bucket), layer_properties });
            }
        }
        Some(LayoutResult { layer_render_data, feature_index: Arc::new(FeatureIndex::read(r)?) })
    }
}
