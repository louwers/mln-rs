//! How the crate's layouts order a tile's features and ask for their
//! patterns.

use std::sync::Arc;

use mln_render::layer::{EvaluatedLayer, FeaturePatterns, LayoutCx};
use mln_style::evaluated::Evaluated;
use mln_style::expression::{EvaluationContext, Image};
use mln_style::layer_type::{PaintSet, StyleLayerType};
use mln_util::error::Error;

/// Picks a layer type's `*-pattern` property of its evaluated paint values.
pub(crate) type Pattern<S> = fn(&<<S as StyleLayerType>::PaintProperties as PaintSet>::Evaluated) -> &Evaluated<Image>;

/// Whether a layer of `group` has a pattern, whose images its layout waits
/// for.
pub(crate) fn uses_pattern<S: StyleLayerType>(group: &[Arc<EvaluatedLayer<S>>], pattern: Pattern<S>) -> bool {
    group.iter().any(|layer| {
        let value = pattern(&layer.paint);
        !value.is_constant() || !value.faded().to.id().is_empty()
    })
}

/// The features of `cx`'s tile that its leader's filter selects, by index,
/// each with its images of the patterns that `pattern` picks, which `cx`
/// asks for, in the order of `key`, the leader's sort key where its style
/// defines one ([`sort`]).
pub(crate) fn with_patterns<S: StyleLayerType>(
    cx: &mut LayoutCx<'_, S>,
    pattern: Pattern<S>,
    key: Option<&Evaluated<f32>>,
) -> Result<Vec<(usize, FeaturePatterns)>, Error> {
    let selected = cx.selected()?;
    let patterns = cx.need_patterns(pattern, &selected)?;
    let (zoom, canonical) = (f32::from(cx.tile().overscaled_z), cx.tile().canonical);
    sort(selected.into_iter().zip(patterns).collect(), key, |key, i| {
        let feature = cx.feature(i)?;
        let context = EvaluationContext::with_zoom_feature(zoom, Some(&feature));
        Ok(key.evaluate(&context.with_canonical_tile_id(Some(&canonical))))
    })
}

/// Orders `features`, a tile's selected features in feature order, each its
/// index with what goes with it, by `key`, the group leader's sort key where
/// its style defines one. Where the key differs by feature, `value(key,
/// index)` gives each feature's, and a feature goes before the first one
/// whose value isn't less; where the key is the same for every feature,
/// each goes before the earlier ones, as inserting it would put it.
pub(crate) fn sort<T>(
    features: Vec<(usize, T)>,
    key: Option<&Evaluated<f32>>,
    mut value: impl FnMut(&Evaluated<f32>, usize) -> Result<f32, Error>,
) -> Result<Vec<(usize, T)>, Error> {
    let Some(key) = key else { return Ok(features) };
    if key.constant().is_some() {
        let mut features = features;
        features.reverse();
        return Ok(features);
    }
    let mut sorted: Vec<(f32, (usize, T))> = Vec::with_capacity(features.len());
    for (index, with) in features {
        let value = value(key, index)?;
        sorted.insert(sorted.partition_point(|&(other, _)| other < value), (value, (index, with)));
    }
    Ok(sorted.into_iter().map(|(_, feature)| feature).collect())
}
