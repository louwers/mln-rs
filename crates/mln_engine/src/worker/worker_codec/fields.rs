// How the members of the tile workers' messages that the derive can't
// write cross (mod.rs): trait objects, as the types they can be, and the
// values that cross once per worker instance, then by key
// (mln_wire::shared).

use std::sync::Arc;

use mln_style::wire::{SharedExpression, SharedLayer};
use mln_vec_map::VecSet;
use mln_wire::shared::{self, ErasedTable, Role, Table};
use mln_wire::{Context, Reader, Request, Wire, Writer};

use crate::Registry;
use crate::layer::bucket::Bucket;
use crate::layer::layer_type::ErasedEvaluatedLayer;
use crate::query::feature_index::FeatureIndex;
use crate::tile::cut::SharedAncestor;
use crate::tile_store::contents::{LayoutBuckets, LayoutResult};
use crate::worker::job::SourceStyle;

/// The `shared::Kind` of layers' evaluated values: their type's name, then
/// the values, which the reading side reads as the type that the registry
/// in its context has by the name.
pub(crate) struct SharedEvaluatedLayer;

impl shared::Kind for SharedEvaluatedLayer {
    type Value = dyn ErasedEvaluatedLayer;

    fn write(layer: &dyn ErasedEvaluatedLayer, w: &mut Writer) {
        mln_wire::write_str(w, layer.style().info().name);
        layer.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<dyn ErasedEvaluatedLayer>> {
        let name = mln_wire::read_str(r)?;
        let layer_type = match r.context::<Registry>() {
            None => Err("a layer without a registry to resolve its type against"),
            Some(registry) => registry.layer_type(name).ok_or("unknown layer type"),
        };
        match layer_type {
            Ok(layer_type) => layer_type.read_evaluated(r),
            Err(message) => r.fail(message),
        }
    }
}

/// The `shared::Kind` of a source's style: its version, its layers each by
/// key, its global state and font faces.
pub(crate) struct SharedSourceStyle;

impl shared::Kind for SharedSourceStyle {
    type Value = SourceStyle;

    fn write(style: &SourceStyle, w: &mut Writer) {
        style.version.write(w);
        style.layers.len().write(w);
        for layer in &style.layers {
            shared::write::<SharedEvaluatedLayer>(layer, w);
        }
        style.global_state.write(w);
        style.font_faces.write(w);
        style.glyph_pbfs.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<SourceStyle>> {
        let version = u64::read(r)?;
        let len = r.length()?;
        let mut layers = Vec::with_capacity(len.min(r.rest().len()));
        for _ in 0..len {
            layers.push(shared::read::<SharedEvaluatedLayer>(r)?);
        }
        Some(Arc::new(SourceStyle {
            version,
            layers,
            global_state: Wire::read(r)?,
            font_faces: Wire::read(r)?,
            glyph_pbfs: Wire::read(r)?,
        }))
    }
}

/// The `shared::Kind` of the sets of images that exist.
pub(crate) struct SharedAvailableImages;

impl shared::Kind for SharedAvailableImages {
    type Value = VecSet<String>;

    fn write(images: &VecSet<String>, w: &mut Writer) {
        images.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Arc<VecSet<String>>> {
        VecSet::read(r).map(Arc::new)
    }
}

/// One side's tables of the values that cross once per worker instance,
/// one per kind. As a context, it gives out each table.
pub(crate) struct Tables {
    source_styles: Table<SharedSourceStyle>,
    available_images: Table<SharedAvailableImages>,
    layers: Table<SharedLayer>,
    evaluated_layers: Table<SharedEvaluatedLayer>,
    expressions: Table<SharedExpression>,
    ancestors: Table<SharedAncestor>,
}

impl Tables {
    pub(crate) fn new(role: Role) -> Self {
        Tables {
            source_styles: Table::new(role),
            available_images: Table::new(role),
            layers: Table::new(role),
            evaluated_layers: Table::new(role),
            expressions: Table::new(role),
            ancestors: Table::new(role),
        }
    }

    /// The tables, in the order the lists of keys to forget are in.
    fn all(&mut self) -> [&mut dyn ErasedTable; 6] {
        [
            &mut self.source_styles,
            &mut self.available_images,
            &mut self.layers,
            &mut self.evaluated_layers,
            &mut self.expressions,
            &mut self.ancestors,
        ]
    }

    /// Keeps the keys given while writing a message if it was `sent`, and
    /// takes them back otherwise, so that a message that wasn't sent
    /// defines nothing.
    pub(crate) fn settle(&mut self, sent: bool) {
        for table in self.all() {
            if sent { table.commit() } else { table.rollback() }
        }
    }

    /// The keys of the values that died on this side, by table, for the
    /// other side to forget.
    pub(crate) fn forget_dead(&mut self) -> Vec<Vec<u64>> {
        self.all().into_iter().map(ErasedTable::forget_dead).collect()
    }

    /// Drops the copies of the values the other side lists as dead
    /// (`forget_dead`).
    pub(crate) fn forget(&mut self, dead: &[Vec<u64>]) {
        for (table, keys) in self.all().into_iter().zip(dead) {
            table.forget(keys);
        }
    }
}

impl Context for Tables {
    fn provide<'a>(&'a mut self, request: &mut Request<'a>) {
        for table in self.all() {
            table.provide(request);
        }
    }
}

/// A bucket and the ids of the layers that render it with the properties
/// they were laid out with.
type RenderedBucket<'a> = (&'a dyn Bucket, Vec<(&'a String, &'a Arc<dyn ErasedEvaluatedLayer>)>);

impl LayoutResult {
    /// Each bucket with the layers that render it, in layer id order.
    fn buckets(&self) -> Vec<RenderedBucket<'_>> {
        let laid = &self.layer_render_data;
        // Where each of the layout's buckets is among `buckets`.
        let mut positions: Vec<Option<usize>> = vec![None; laid.bucket_count()];
        let mut buckets: Vec<RenderedBucket<'_>> = Vec::new();
        let mut layers: Vec<_> = laid.laid_layers().collect();
        layers.sort_unstable_by_key(|(layer_id, ..)| *layer_id);
        for (layer_id, bucket_index, layer) in layers {
            let Some(bucket) = laid.bucket(bucket_index) else { continue };
            let position = *positions[bucket_index].get_or_insert_with(|| {
                buckets.push((bucket, Vec::new()));
                buckets.len() - 1
            });
            buckets[position].1.push((layer_id, layer));
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
                shared::write::<SharedEvaluatedLayer>(properties, w);
            }
            // The reader reads the bucket as its layers' type's.
            bucket.write_wire(w);
        }
    }
}

/// The encoding of a layout's buckets with the layers that render them, as
/// `LayoutResult` writes them without the tables in the context.
#[cfg(test)]
pub(crate) fn encode_buckets(result: &LayoutResult) -> Vec<u8> {
    let mut w = Writer::new();
    result.write_buckets(&mut w);
    w.finish().unwrap()
}

/// A `LayoutResult` crosses as each bucket once, after the layers that
/// render it (the layers of a layout group share their bucket, and their type is
/// the bucket's), and the feature index without its data.
impl Wire for LayoutResult {
    fn write(&self, w: &mut Writer) {
        self.write_buckets(w);
        self.feature_index.write(w);
    }

    fn read(r: &mut Reader<'_>) -> Option<Self> {
        let mut layer_render_data = LayoutBuckets::default();
        let buckets = r.length()?;
        for _ in 0..buckets {
            let len = r.length()?;
            let mut layers = Vec::with_capacity(len.min(r.rest().len()));
            for _ in 0..len {
                layers.push((String::read(r)?, shared::read::<SharedEvaluatedLayer>(r)?));
            }
            let Some((_, leader)) = layers.first() else {
                return r.fail("a bucket that no layer renders");
            };
            let bucket = leader.entry().read_bucket(r)?;
            layer_render_data.add(bucket, layers);
        }
        Some(LayoutResult { layer_render_data, feature_index: Arc::new(FeatureIndex::read(r)?) })
    }
}
