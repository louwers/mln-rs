// The render half of the layer manager: `create_render_layer`,
// `create_bucket` and `create_layout`. The style half (`create_layer`) is
// `mln_style::layermanager`. They dispatch on the layer's `LayerTypeInfo`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use mln_style::layer;
use mln_style::layer::{downcast_arc, layer_type_info};
use mln_style::layer_properties::LayerProperties;
use mln_style::layers::background_layer_impl::Impl as BackgroundLayerImpl;
use mln_style::layers::circle_layer_impl::Impl as CircleLayerImpl;
use mln_style::layers::custom_layer::Impl as CustomLayerImpl;
use mln_style::layers::fill_extrusion_layer_impl::Impl as FillExtrusionLayerImpl;
use mln_style::layers::fill_layer_impl::Impl as FillLayerImpl;
use mln_style::layers::line_layer_impl::Impl as LineLayerImpl;
use mln_style::layers::raster_layer_impl::Impl as RasterLayerImpl;
use mln_style::layers::symbol_layer_impl::Impl as SymbolLayerImpl;
use mln_util::error::Error;
use mln_util::logging::{Event, Log};

use crate::bucket::Bucket;
use crate::bucket_parameters::BucketParameters;
use crate::buckets::fill_extrusion_bucket::FillExtrusionLayoutSpec;
use crate::layers::render_custom_layer::RenderCustomLayer;
use crate::layers::{
    RenderBackgroundLayer, RenderCircleLayer, RenderFillExtrusionLayer, RenderFillLayer, RenderLineLayer,
    RenderRasterLayer,
};
use crate::layout::circle_layout::CircleLayout;
use crate::layout::layout::{Layout, LayoutParameters, SourceLayer};
use crate::layout::pattern_layout::{
    FillLayoutSpec, LineLayoutSpec, PatternLayout, fill_layout_is_sorted, line_layout_is_sorted,
};
use crate::render_layer::RenderLayerPtr;

pub fn create_render_layer(impl_: &Arc<dyn layer::Impl>) -> Option<RenderLayerPtr> {
    let type_info = impl_.get_type_info();
    if std::ptr::eq(type_info, CustomLayerImpl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(RenderCustomLayer::new(downcast_arc::<CustomLayerImpl>(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, BackgroundLayerImpl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(RenderBackgroundLayer::new(downcast_arc::<BackgroundLayerImpl>(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, CircleLayerImpl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(RenderCircleLayer::new(downcast_arc::<CircleLayerImpl>(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, FillLayerImpl::static_type_info()) {
        let layer: RenderLayerPtr = Rc::new(RefCell::new(RenderFillLayer::new(downcast_arc::<FillLayerImpl>(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, LineLayerImpl::static_type_info()) {
        let layer: RenderLayerPtr = Rc::new(RefCell::new(RenderLineLayer::new(downcast_arc::<LineLayerImpl>(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, FillExtrusionLayerImpl::static_type_info()) {
        // conformance: FillExtrusionLayerFactory::createRenderLayer
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(RenderFillExtrusionLayer::new(downcast_arc::<FillExtrusionLayerImpl>(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, RasterLayerImpl::static_type_info()) {
        // conformance: RasterLayerFactory::createRenderLayer
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(RenderRasterLayer::new(downcast_arc::<RasterLayerImpl>(impl_))));
        return Some(layer);
    }
    // dem-heatmap: HeatmapLayerFactory::createRenderLayer
    if std::ptr::eq(type_info, mln_style::layers::heatmap_layer_impl::Impl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(crate::layers::render_heatmap_layer::RenderHeatmapLayer::new(downcast_arc::<
                mln_style::layers::heatmap_layer_impl::Impl,
            >(impl_))));
        return Some(layer);
    }
    // dem-heatmap: ColorReliefLayerFactory::createRenderLayer
    if std::ptr::eq(type_info, mln_style::layers::color_relief_layer_impl::Impl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(crate::layers::render_color_relief_layer::RenderColorReliefLayer::new(
                downcast_arc::<mln_style::layers::color_relief_layer_impl::Impl>(impl_),
            )));
        return Some(layer);
    }
    // dem-heatmap: HillshadeLayerFactory::createRenderLayer
    if std::ptr::eq(type_info, mln_style::layers::hillshade_layer_impl::Impl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(crate::layers::render_hillshade_layer::RenderHillshadeLayer::new(downcast_arc::<
                mln_style::layers::hillshade_layer_impl::Impl,
            >(
                impl_
            ))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, SymbolLayerImpl::static_type_info()) {
        // symbols: SymbolLayerFactory::createRenderLayer
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(crate::layers::RenderSymbolLayer::new(downcast_arc::<SymbolLayerImpl>(impl_))));
        return Some(layer);
    }
    Log::error(Event::Render, format!("no render layer for layer type '{}'", type_info.type_));
    None
}

/// Creates the bucket of a layer type that has no layout.
pub(crate) fn create_bucket(
    parameters: &BucketParameters,
    layers: &[Arc<dyn LayerProperties>],
) -> Option<Box<dyn Bucket>> {
    debug_assert!(!layers.is_empty());
    debug_assert_eq!(parameters.layer_type.layout, layer_type_info::Layout::NotRequired);
    // dem-heatmap: HeatmapLayerFactory::createBucket
    if std::ptr::eq(parameters.layer_type, mln_style::layers::heatmap_layer_impl::Impl::static_type_info()) {
        return Some(Box::new(crate::buckets::heatmap_bucket::HeatmapBucket::new(parameters, layers)));
    }
    Log::error(Event::Render, format!("no bucket for layer type '{}'", parameters.layer_type.type_));
    None
}

pub(crate) fn create_layout(
    parameters: &mut LayoutParameters<'_>,
    tile_layer: SourceLayer,
    layers: &[Arc<dyn LayerProperties>],
) -> Result<Option<Box<dyn Layout>>, Error> {
    debug_assert!(!layers.is_empty());
    debug_assert_eq!(parameters.bucket_parameters.layer_type.layout, layer_type_info::Layout::Required);
    let type_info = parameters.bucket_parameters.layer_type;
    if std::ptr::eq(type_info, CircleLayerImpl::static_type_info()) {
        // CircleLayerFactory::createLayout
        return Ok(Some(Box::new(CircleLayout::with_source_layer(
            parameters.bucket_parameters,
            layers,
            tile_layer,
            parameters.filter_groups,
        )?)));
    }
    if std::ptr::eq(type_info, FillLayerImpl::static_type_info()) {
        // FillLayerFactory::createLayout: the FillSortKey instantiation
        // unless the leader's sort key is undefined.
        let sorted = fill_layout_is_sorted(&layers[0]);
        return Ok(Some(Box::new(PatternLayout::<FillLayoutSpec>::with_source_layer(
            parameters, layers, tile_layer, sorted,
        )?)));
    }
    if std::ptr::eq(type_info, LineLayerImpl::static_type_info()) {
        // LineLayerFactory::createLayout
        let sorted = line_layout_is_sorted(&layers[0]);
        return Ok(Some(Box::new(PatternLayout::<LineLayoutSpec>::with_source_layer(
            parameters, layers, tile_layer, sorted,
        )?)));
    }
    if std::ptr::eq(type_info, FillExtrusionLayerImpl::static_type_info()) {
        // conformance: FillExtrusionLayerFactory::createLayout (no sort key)
        return Ok(Some(Box::new(PatternLayout::<FillExtrusionLayoutSpec>::with_source_layer(
            parameters, layers, tile_layer, false,
        )?)));
    }
    if std::ptr::eq(type_info, SymbolLayerImpl::static_type_info()) {
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
    Log::error(Event::Render, format!("no layout for layer type '{}'", parameters.bucket_parameters.layer_type.type_));
    Ok(None)
}
