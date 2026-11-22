use std::sync::Arc;

use mln_render::layer::{BuildCx, EvaluatedLayer, FeaturePatterns, Layout, LayoutCx};
use mln_style::StyleValue as _;
use mln_util::error::Error;
use mln_util::logging::{Event, Log};

use super::Fill;
use super::bucket::FillBucket;
use crate::features::{uses_pattern, with_patterns};
use crate::tessellator::Tessellator;

/// The features a tile's fills draw, by index, in `fill-sort-key` order,
/// each with its patterns.
pub struct FillLayout(Vec<(usize, FeaturePatterns)>);

impl Layout<Fill> for FillLayout {
    type Bucket = FillBucket;

    /// Whether a layer of the group has a pattern, whose images it waits
    /// for.
    fn needs_resources(group: &[Arc<EvaluatedLayer<Fill>>]) -> bool {
        uses_pattern(group, |paint| &paint.fill_pattern)
    }

    fn new(cx: &mut LayoutCx<'_, Fill>) -> Result<Self, Error> {
        let layout = &cx.layers()[0].style.layout;
        let key = (!layout.fill_sort_key.is_undefined()).then(|| layout.evaluate(cx.parameters()).fill_sort_key);
        Ok(FillLayout(with_patterns(cx, |paint| &paint.fill_pattern, key.as_ref())?))
    }

    fn build(self, cx: &mut BuildCx<'_, Fill>) -> Result<Option<FillBucket>, Error> {
        let tile = cx.tile();
        let granularity = cx.subdivision_granularity().fill.granularity_for_zoom_level(tile.z);
        let mut bucket = FillBucket { binders: cx.binders(), ..FillBucket::default() };
        let mut sdf = Sdf::default();
        for layer in cx.layers() {
            let pattern = &layer.paint.fill_pattern;
            if pattern.is_constant() {
                let faded = pattern.faded();
                sdf.record(cx, &layer.style.id, [faded.from.id(), faded.to.id()]);
            }
        }
        let mut tessellator = Tessellator::default();
        for (i, patterns) in &self.0 {
            for layer in cx.layers() {
                if let Some(images) = patterns.images(&layer.style.id) {
                    sdf.record(cx, &layer.style.id, images);
                }
            }
            let feature = cx.feature(*i)?;
            let geometry = feature.geometry()?;
            bucket.add(&geometry, &tile, granularity, &mut tessellator);
            cx.populate_patterns(&mut bucket.binders, *i, &feature, bucket.vertices.len(), patterns);
            cx.index(*i, &geometry);
        }
        bucket.sdf = sdf.layers;
        Ok(Some(bucket))
    }
}

/// Whether each layer's patterns are SDF images: the first of its
/// patterns that its layout resolved says.
#[derive(Default)]
struct Sdf {
    layers: Vec<(String, bool)>,
    /// The layers whose patterns mix SDF and other images, which a warning
    /// named.
    mixed: Vec<String>,
}

impl Sdf {
    /// Records the images `images` of the layer `layer`'s patterns that
    /// `cx`'s layout resolved.
    fn record<'a>(&mut self, cx: &BuildCx<'_, Fill>, layer: &str, images: impl IntoIterator<Item = &'a str>) {
        let Some(resolved) = cx.resolved() else { return };
        for sdf in images.into_iter().filter_map(|id| Some(resolved.patterns.get(id)?.sdf)) {
            match self.layers.iter().find(|(id, _)| id == layer) {
                None => self.layers.push((layer.to_owned(), sdf)),
                Some(&(_, first)) if first != sdf && !self.mixed.iter().any(|id| id == layer) => {
                    self.mixed.push(layer.to_owned());
                    Log::warning(
                        Event::Style,
                        format!("Style sheet warning: Cannot mix SDF and non-SDF fill patterns in layer \"{layer}\""),
                    );
                }
                Some(_) => {}
            }
        }
    }
}
