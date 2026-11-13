// Ports the render half of src/mln/layermanager/layer_manager.cpp and the
// layer factories (`createBucket`, `createLayout`, `createRenderLayer`).
// The style half (`createLayer`) is mln_style::layermanager; the dispatch
// here is on the layer's `LayerTypeInfo`, as `LayerManager::findFactory`
// does.

use std::cell::RefCell;
use std::rc::Rc;

use mln_style::layer;
use mln_style::layer::{layer_type_info, static_immutable_cast};
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
use mln_util::immutable::Immutable;
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

/// `LayerManager::createRenderLayer(Immutable<style::Layer::Impl>)`.
pub fn create_render_layer(impl_: &Immutable<dyn layer::Impl>) -> Option<RenderLayerPtr> {
    let type_info = impl_.get_type_info();
    if std::ptr::eq(type_info, CustomLayerImpl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(RenderCustomLayer::new(static_immutable_cast::<CustomLayerImpl>(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, BackgroundLayerImpl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(RenderBackgroundLayer::new(static_immutable_cast::<BackgroundLayerImpl>(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, CircleLayerImpl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(RenderCircleLayer::new(static_immutable_cast::<CircleLayerImpl>(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, FillLayerImpl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(RenderFillLayer::new(static_immutable_cast::<FillLayerImpl>(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, LineLayerImpl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(RenderLineLayer::new(static_immutable_cast::<LineLayerImpl>(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, FillExtrusionLayerImpl::static_type_info()) {
        // conformance: FillExtrusionLayerFactory::createRenderLayer
        let layer: RenderLayerPtr = Rc::new(RefCell::new(RenderFillExtrusionLayer::new(static_immutable_cast::<
            FillExtrusionLayerImpl,
        >(impl_))));
        return Some(layer);
    }
    if std::ptr::eq(type_info, RasterLayerImpl::static_type_info()) {
        // conformance: RasterLayerFactory::createRenderLayer
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(RenderRasterLayer::new(static_immutable_cast::<RasterLayerImpl>(impl_))));
        return Some(layer);
    }
    // dem-heatmap: HeatmapLayerFactory::createRenderLayer
    if std::ptr::eq(type_info, mln_style::layers::heatmap_layer_impl::Impl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(crate::layers::render_heatmap_layer::RenderHeatmapLayer::new(
                static_immutable_cast::<mln_style::layers::heatmap_layer_impl::Impl>(impl_),
            )));
        return Some(layer);
    }
    // dem-heatmap: ColorReliefLayerFactory::createRenderLayer
    if std::ptr::eq(type_info, mln_style::layers::color_relief_layer_impl::Impl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(crate::layers::render_color_relief_layer::RenderColorReliefLayer::new(
                static_immutable_cast::<mln_style::layers::color_relief_layer_impl::Impl>(impl_),
            )));
        return Some(layer);
    }
    // dem-heatmap: HillshadeLayerFactory::createRenderLayer
    if std::ptr::eq(type_info, mln_style::layers::hillshade_layer_impl::Impl::static_type_info()) {
        let layer: RenderLayerPtr =
            Rc::new(RefCell::new(crate::layers::render_hillshade_layer::RenderHillshadeLayer::new(
                static_immutable_cast::<mln_style::layers::hillshade_layer_impl::Impl>(impl_),
            )));
        return Some(layer);
    }
    if std::ptr::eq(type_info, SymbolLayerImpl::static_type_info()) {
        // symbols: SymbolLayerFactory::createRenderLayer
        let layer: RenderLayerPtr = Rc::new(RefCell::new(crate::layers::RenderSymbolLayer::new(
            static_immutable_cast::<SymbolLayerImpl>(impl_),
        )));
        return Some(layer);
    }
    // TODO(port): the other layer types (each arrives with its RenderLayer).
    Log::error(Event::Render, format!("TODO(port): no render layer for layer type '{}'", type_info.type_));
    None
}

/// `LayerManager::createBucket(parameters, layers)`: the layer types without
/// a layout.
pub fn create_bucket(
    parameters: &BucketParameters,
    layers: &[Immutable<dyn LayerProperties>],
) -> Option<Box<dyn Bucket>> {
    debug_assert!(!layers.is_empty());
    debug_assert_eq!(parameters.layer_type.layout, layer_type_info::Layout::NotRequired);
    // dem-heatmap: HeatmapLayerFactory::createBucket
    if std::ptr::eq(parameters.layer_type, mln_style::layers::heatmap_layer_impl::Impl::static_type_info()) {
        return Some(Box::new(crate::buckets::heatmap_bucket::HeatmapBucket::new(parameters, layers)));
    }
    Log::error(Event::Render, format!("TODO(port): no bucket for layer type '{}'", parameters.layer_type.type_));
    None
}

/// `LayerManager::createLayout(parameters, tileLayer, layers)`.
pub fn create_layout(
    parameters: &mut LayoutParameters<'_>,
    tile_layer: SourceLayer,
    layers: &[Immutable<dyn LayerProperties>],
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
    Log::error(
        Event::Render,
        format!("TODO(port): no layout for layer type '{}'", parameters.bucket_parameters.layer_type.type_),
    );
    Ok(None)
}
