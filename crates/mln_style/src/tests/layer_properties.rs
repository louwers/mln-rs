//! `getEvaluated` / `getCrossfade` (`include/mln/style/layer_properties.hpp`)
//! on a real `BackgroundLayerProperties` behind `Arc<dyn LayerProperties>`.
//! No C++ gtest; the renderer reads evaluated properties this way.

use std::sync::Arc;

use mln_util::Color;

use crate::layer_properties::{LayerProperties, get_crossfade, get_evaluated};
use crate::layers::background_layer::Impl as BackgroundLayerImpl;
use crate::layers::{BackgroundLayerProperties, BackgroundPaintProperties};
use crate::renderer::property_evaluation_parameters::CrossfadeParameters;

fn background_properties() -> Arc<dyn LayerProperties> {
    let evaluated = BackgroundPaintProperties::PossiblyEvaluated {
        background_color: Color::new(0.25, 0.5, 0.75, 1.0),
        background_opacity: 0.5,
        ..BackgroundPaintProperties::PossiblyEvaluated::default()
    };
    let crossfade = CrossfadeParameters { from_scale: 2.0, to_scale: 1.0, t: 0.25 };
    let impl_ = Arc::new(BackgroundLayerImpl::new("background".to_owned(), String::new()));
    Arc::new(BackgroundLayerProperties::with_evaluated(impl_, crossfade, evaluated))
}

#[test]
fn get_evaluated_downcasts_the_pointee() {
    let properties = background_properties();
    let evaluated = get_evaluated::<BackgroundLayerProperties>(&properties);
    assert_eq!(evaluated.background_color, Color::new(0.25, 0.5, 0.75, 1.0));
    assert_eq!(evaluated.background_opacity, 0.5);
}

#[test]
fn get_crossfade_downcasts_the_pointee() {
    let properties = background_properties();
    let crossfade = get_crossfade::<BackgroundLayerProperties>(&properties);
    assert_eq!(crossfade.from_scale, 2.0);
    assert_eq!(crossfade.to_scale, 1.0);
    assert_eq!(crossfade.t, 0.25);
}
