// How tiles lay out the layer types: `create_bucket` and `create_layout`,
// which dispatch on the layer type's name.

use std::sync::Arc;

use mln_style::layer::{LayerTypeInfo, SourceKind};
use mln_style::layer_type::StyleLayerType;
use mln_style::layers::{Circle, Fill, FillExtrusion, Heatmap, Line, Symbol};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};

use crate::bucket::Bucket;
use crate::bucket_parameters::BucketParameters;
use crate::buckets::fill_extrusion_bucket::FillExtrusionLayoutSpec;
use crate::layer_type::ErasedEvaluatedLayer;
use crate::layout::circle_layout::CircleLayout;
use crate::layout::layout::{Layout, LayoutParameters, SourceLayer};
use crate::layout::pattern_layout::{
    FillLayoutSpec, LineLayoutSpec, PatternLayout, fill_layout_is_sorted, line_layout_is_sorted, uses_pattern,
};

/// Whether tiles lay out the layer type's layers through a `Layout`; a
/// heatmap's bucket is made without one (`create_bucket`).
pub(crate) fn lays_out(layer_type: LayerTypeInfo) -> bool {
    layer_type.source == SourceKind::Geometry && layer_type.name != Heatmap::INFO.name
}

/// Creates the bucket of a layer type that has no layout.
pub(crate) fn create_bucket(
    parameters: &BucketParameters,
    layers: &[Arc<dyn ErasedEvaluatedLayer>],
) -> Option<Box<dyn Bucket>> {
    debug_assert!(!layers.is_empty());
    debug_assert!(!lays_out(parameters.layer_type));
    if parameters.layer_type.name == Heatmap::INFO.name {
        return Some(Box::new(crate::buckets::heatmap_bucket::HeatmapBucket::new(parameters, layers)));
    }
    Log::error(Event::Render, format!("no bucket for layer type '{}'", parameters.layer_type.name));
    None
}

/// Whether the layout of `layers`, a group of `layer_type`, can need glyphs
/// or images: a symbol group, or a fill, line or fill-extrusion group with a
/// pattern.
pub(crate) fn layout_can_need_dependencies(
    layer_type: LayerTypeInfo,
    layers: &[Arc<dyn ErasedEvaluatedLayer>],
) -> bool {
    let name = layer_type.name;
    if name == Symbol::INFO.name {
        true
    } else if name == Fill::INFO.name {
        uses_pattern::<FillLayoutSpec>(layers)
    } else if name == Line::INFO.name {
        uses_pattern::<LineLayoutSpec>(layers)
    } else if name == FillExtrusion::INFO.name {
        uses_pattern::<FillExtrusionLayoutSpec>(layers)
    } else {
        false
    }
}

pub(crate) fn create_layout(
    parameters: &mut LayoutParameters<'_>,
    tile_layer: SourceLayer,
    layers: &[Arc<dyn ErasedEvaluatedLayer>],
) -> Result<Option<Box<dyn Layout>>, Error> {
    debug_assert!(!layers.is_empty());
    debug_assert!(lays_out(parameters.bucket_parameters.layer_type));
    let name = parameters.bucket_parameters.layer_type.name;
    if name == Circle::INFO.name {
        // CircleLayerFactory::createLayout
        return Ok(Some(Box::new(CircleLayout::with_source_layer(
            parameters.bucket_parameters,
            layers,
            tile_layer,
            parameters.filter_groups,
        )?)));
    }
    if name == Fill::INFO.name {
        // FillLayerFactory::createLayout: the FillSortKey instantiation
        // unless the leader's sort key is undefined.
        let sorted = fill_layout_is_sorted(&layers[0]);
        return Ok(Some(Box::new(PatternLayout::<FillLayoutSpec>::with_source_layer(
            parameters, layers, tile_layer, sorted,
        )?)));
    }
    if name == Line::INFO.name {
        // LineLayerFactory::createLayout
        let sorted = line_layout_is_sorted(&layers[0]);
        return Ok(Some(Box::new(PatternLayout::<LineLayoutSpec>::with_source_layer(
            parameters, layers, tile_layer, sorted,
        )?)));
    }
    if name == FillExtrusion::INFO.name {
        // conformance: FillExtrusionLayerFactory::createLayout (no sort key)
        return Ok(Some(Box::new(PatternLayout::<FillExtrusionLayoutSpec>::with_source_layer(
            parameters, layers, tile_layer, false,
        )?)));
    }
    if name == Symbol::INFO.name {
        // symbols: SymbolLayerFactory::createLayout
        let bucket_parameters = parameters.bucket_parameters;
        return Ok(Some(Box::new(crate::layout::symbol_layout::SymbolLayout::new(
            bucket_parameters,
            layers,
            tile_layer,
            parameters,
        )?)));
    }
    let _ = tile_layer;
    Log::error(Event::Render, format!("no layout for layer type '{name}'"));
    Ok(None)
}
