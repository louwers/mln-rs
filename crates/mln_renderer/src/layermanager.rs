// The render half of the layer types: `create_render_layer`,
// `create_bucket` and `create_layout`, which dispatch on the layer type's
// name. The style half is each type's `mln_style::layer_type::StyleLayerType`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_style::layer;
use mln_style::layer::{LayerTypeInfo, SourceKind, downcast_arc};
use mln_style::layer_properties::LayerProperties;
use mln_style::layer_type::StyleLayerType;
use mln_style::layers::background_layer_impl::Impl as BackgroundLayerImpl;
use mln_style::layers::circle_layer_impl::Impl as CircleLayerImpl;
use mln_style::layers::color_relief_layer_impl::Impl as ColorReliefLayerImpl;
use mln_style::layers::custom_layer::Impl as CustomLayerImpl;
use mln_style::layers::fill_extrusion_layer_impl::Impl as FillExtrusionLayerImpl;
use mln_style::layers::fill_layer_impl::Impl as FillLayerImpl;
use mln_style::layers::heatmap_layer_impl::Impl as HeatmapLayerImpl;
use mln_style::layers::hillshade_layer_impl::Impl as HillshadeLayerImpl;
use mln_style::layers::line_layer_impl::Impl as LineLayerImpl;
use mln_style::layers::raster_layer_impl::Impl as RasterLayerImpl;
use mln_style::layers::symbol_layer_impl::Impl as SymbolLayerImpl;
use mln_style::layers::{
    Background, Circle, ColorRelief, Custom, Fill, FillExtrusion, Heatmap, Hillshade, Line, Raster, Symbol,
};
use mln_util::error::Error;
use mln_util::logging::{Event, Log};

use crate::bucket::Bucket;
use crate::bucket_parameters::BucketParameters;
use crate::buckets::fill_extrusion_bucket::FillExtrusionLayoutSpec;
use crate::layers::render_color_relief_layer::RenderColorReliefLayer;
use crate::layers::render_custom_layer::RenderCustomLayer;
use crate::layers::render_heatmap_layer::RenderHeatmapLayer;
use crate::layers::render_hillshade_layer::RenderHillshadeLayer;
use crate::layers::{
    RenderBackgroundLayer, RenderCircleLayer, RenderFillExtrusionLayer, RenderFillLayer, RenderLineLayer,
    RenderRasterLayer, RenderSymbolLayer,
};
use crate::layout::circle_layout::CircleLayout;
use crate::layout::layout::{Layout, LayoutParameters, SourceLayer};
use crate::layout::pattern_layout::{
    FillLayoutSpec, LineLayoutSpec, PatternLayout, fill_layout_is_sorted, line_layout_is_sorted, uses_pattern,
};
use crate::render_layer::RenderLayerPtr;

pub fn create_render_layer(impl_: &Arc<dyn layer::Impl>) -> Option<RenderLayerPtr> {
    let name = impl_.get_type_info().name;
    let layer: RenderLayerPtr = if name == Custom::INFO.name {
        Rc::new(RefCell::new(RenderCustomLayer::new(downcast_arc::<CustomLayerImpl>(impl_))))
    } else if name == Background::INFO.name {
        Rc::new(RefCell::new(RenderBackgroundLayer::new(downcast_arc::<BackgroundLayerImpl>(impl_))))
    } else if name == Circle::INFO.name {
        Rc::new(RefCell::new(RenderCircleLayer::new(downcast_arc::<CircleLayerImpl>(impl_))))
    } else if name == Fill::INFO.name {
        Rc::new(RefCell::new(RenderFillLayer::new(downcast_arc::<FillLayerImpl>(impl_))))
    } else if name == Line::INFO.name {
        Rc::new(RefCell::new(RenderLineLayer::new(downcast_arc::<LineLayerImpl>(impl_))))
    } else if name == FillExtrusion::INFO.name {
        Rc::new(RefCell::new(RenderFillExtrusionLayer::new(downcast_arc::<FillExtrusionLayerImpl>(impl_))))
    } else if name == Raster::INFO.name {
        Rc::new(RefCell::new(RenderRasterLayer::new(downcast_arc::<RasterLayerImpl>(impl_))))
    } else if name == Heatmap::INFO.name {
        Rc::new(RefCell::new(RenderHeatmapLayer::new(downcast_arc::<HeatmapLayerImpl>(impl_))))
    } else if name == ColorRelief::INFO.name {
        Rc::new(RefCell::new(RenderColorReliefLayer::new(downcast_arc::<ColorReliefLayerImpl>(impl_))))
    } else if name == Hillshade::INFO.name {
        Rc::new(RefCell::new(RenderHillshadeLayer::new(downcast_arc::<HillshadeLayerImpl>(impl_))))
    } else if name == Symbol::INFO.name {
        Rc::new(RefCell::new(RenderSymbolLayer::new(downcast_arc::<SymbolLayerImpl>(impl_))))
    } else {
        Log::error(Event::Render, format!("no render layer for layer type '{name}'"));
        return None;
    };
    Some(layer)
}

/// Whether tiles lay out the layer type's layers through a `Layout`; a
/// heatmap's bucket is made without one (`create_bucket`).
pub(crate) fn lays_out(layer_type: LayerTypeInfo) -> bool {
    layer_type.source == SourceKind::Geometry && layer_type.name != Heatmap::INFO.name
}

/// Creates the bucket of a layer type that has no layout.
pub(crate) fn create_bucket(
    parameters: &BucketParameters,
    layers: &[Arc<dyn LayerProperties>],
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
pub(crate) fn layout_can_need_dependencies(layer_type: LayerTypeInfo, layers: &[Arc<dyn LayerProperties>]) -> bool {
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
    layers: &[Arc<dyn LayerProperties>],
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
