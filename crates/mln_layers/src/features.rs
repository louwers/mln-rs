//! How the crate's layouts order a tile's features and ask for their
//! patterns, and how the patterns draw.

use std::sync::Arc;

use mln_gfx::texture2d::SamplerState;
use mln_gfx::types::{TextureFilterType, TextureWrapType};
use mln_render::layer::{EvaluatedLayer, FeaturePatterns, LayoutCx};
use mln_style::evaluated::Evaluated;
use mln_style::expression::{EvaluationContext, Image};
use mln_style::layer_type::{PaintSet, StyleLayerType};
use mln_util::error::Error;

/// How a fill's or a line's pattern samples its tile's atlas: linearly,
/// within the pattern's padding.
pub(crate) const PATTERN_SAMPLER: SamplerState = SamplerState {
    filter: TextureFilterType::Linear,
    wrap_u: TextureWrapType::Clamp,
    wrap_v: TextureWrapType::Clamp,
    max_anisotropy: 1,
    mipmapped: false,
};

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

#[cfg(test)]
mod tests {
    use mln_style::PropertyEvaluationParameters;
    use mln_style::conversion::{Convertible, Error as ConversionError, convert_layer};
    use mln_style::layer_type::LayerStyle;
    use serde_json::json;

    use super::*;
    use crate::Circle;

    /// A circle layer's `circle-sort-key` of `value`, evaluated at zoom 0.
    fn sort_key(value: serde_json::Value) -> Evaluated<f32> {
        let json = json!({"id": "circle", "type": "circle", "source": "s", "layout": {"circle-sort-key": value}});
        let mut error = ConversionError::default();
        let layer = convert_layer(crate::tests::registry().style(), &Convertible::from(&json), &mut error)
            .unwrap_or_else(|| panic!("{}", error.message));
        let style = LayerStyle::<Circle>::recover_ref(&**layer.style()).expect("a circle layer");
        style.layout.evaluate(&PropertyEvaluationParameters::new(0.0)).circle_sort_key
    }

    // Ports test/layout/sort_key.test.cpp's fill, line and circle cases,
    // whose layouts share the order.
    #[test]
    fn features_go_in_their_sort_keys_order_and_reverse_where_it_is_the_same_for_all() {
        // Three features, ranked 2, 0 and 1.
        let ranks = [2.0, 0.0, 1.0];
        for (key, order) in [
            (None, [0, 1, 2]),
            (Some(json!(5)), [2, 1, 0]),
            (Some(json!(["step", ["zoom"], 5, 10, 10])), [2, 1, 0]),
            (Some(json!(["number", ["get", "rank"]])), [1, 2, 0]),
        ] {
            let key = key.map(sort_key);
            let features = (0..3).map(|i| (i, ())).collect();
            let sorted = sort(features, key.as_ref(), |_, i| Ok(ranks[i])).unwrap();
            assert_eq!(sorted.iter().map(|&(i, ())| i).collect::<Vec<_>>(), order, "{key:?}");
        }
    }
}
