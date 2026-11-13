// Ports include/mln/renderer/renderer.hpp and src/mln/renderer/renderer.cpp.
//
// C++ keeps a `gfx::RendererBackend&`; the frontend owns the backend, so the
// renderer shares it through an `Rc`.

use std::rc::Rc;
use std::sync::Arc;

use mln_gfx::{BackendScope, FrameTarget, Handoff, RendererBackend, ScopeType};
use mln_text::GlyphCache;

use crate::renderer_event::RendererEvents;
use crate::renderer_impl::RendererImpl;
use crate::tile::tile_worker_transport::TileWorkerTransport;
use crate::update_parameters::UpdateParameters;

pub struct Renderer {
    /// Always `Some` until `drop`, which destroys the impl inside a
    /// `BackendScope` as C++ does.
    impl_: Option<Box<RendererImpl>>,
}

impl Renderer {
    /// `Renderer(gfx::RendererBackend&, float pixelRatio, const
    /// std::optional<std::string>& localFontFamily = std::nullopt)`: a
    /// renderer with a glyph cache of its own. `local_font_family` is the
    /// platform's `LocalGlyphRasterizer`'s (see `DefaultLocalGlyphRasterizer`).
    pub fn new(backend: Rc<dyn RendererBackend>, pixel_ratio: f32, local_font_family: Option<String>) -> Self {
        Self::with_glyph_cache(backend, pixel_ratio, local_font_family, Arc::default())
    }

    /// `Renderer(gfx::RendererBackend&, float pixelRatio, localFontFamily,
    /// std::shared_ptr<GlyphCache>)`: renderers given the same `GlyphCache`
    /// share the glyphs they parse from glyph PBFs.
    pub fn with_glyph_cache(
        backend: Rc<dyn RendererBackend>,
        pixel_ratio: f32,
        local_font_family: Option<String>,
        glyph_cache: Arc<GlyphCache>,
    ) -> Self {
        Renderer { impl_: Some(Box::new(RendererImpl::new(backend, pixel_ratio, local_font_family, glyph_cache))) }
    }

    fn impl_ref(&self) -> &RendererImpl {
        self.impl_.as_deref().expect("renderer impl")
    }

    fn impl_mut(&mut self) -> &mut RendererImpl {
        self.impl_.as_deref_mut().expect("renderer impl")
    }

    /// Draws at `pixel_ratio` device pixels per logical pixel from the next
    /// frame on, which should be the map's (`Map::set_pixel_ratio`). The
    /// renderer makes its shaders and drawables again, and keeps its
    /// tiles. The host sizes the backend's framebuffer for the new ratio.
    pub fn set_pixel_ratio(&mut self, pixel_ratio: f32) {
        let backend = self.impl_ref().backend.clone();
        let _guard = BackendScope::new(&*backend, ScopeType::Explicit);
        self.impl_mut().set_pixel_ratio(pixel_ratio);
    }

    #[must_use]
    pub fn get_pixel_ratio(&self) -> f32 {
        self.impl_ref().pixel_ratio()
    }

    /// Port-only: the transport that creates the tile workers of tiles
    /// created from now on (by default on the map's thread pool). Set it
    /// before the first render so that every tile uses it.
    pub fn set_tile_worker_transport(&mut self, transport: Rc<dyn TileWorkerTransport>) {
        self.impl_mut().orchestrator.set_tile_worker_transport(transport);
    }

    /// Sets the queue the renderer pushes its events to: the map's, which
    /// the map's frontend passes on.
    pub fn set_event_queue(&mut self, events: Option<RendererEvents>) {
        self.impl_mut().orchestrator.set_event_queue(events);
    }

    /// Draws `update_parameters` into a frame of `target`, which must be
    /// built on the renderer's backend. Returns where the frame went, or
    /// `None` when nothing was drawn: there was nothing to draw, or the
    /// target had nothing to draw into.
    pub fn render(&mut self, update_parameters: &Arc<UpdateParameters>, target: &dyn FrameTarget) -> Option<Handoff> {
        let impl_ = self.impl_mut();
        let style_changed = impl_.style_loaded && !update_parameters.style_loaded;
        impl_.style_loaded = update_parameters.style_loaded;
        if style_changed {
            impl_.orchestrator.reset_dynamic_texture_atlas();
        }
        let mut render_tree = impl_.orchestrator.create_render_tree(update_parameters)?;
        render_tree.prepare();
        impl_.render(&render_tree, update_parameters, target)
    }

    /// Where the last frame that [`render`](Self::render) drew put the map,
    /// or `None` before the first.
    #[must_use]
    pub fn frame_projection(&self) -> Option<crate::FrameProjection> {
        self.impl_ref().frame_projection()
    }

    // query: `Err` is the exception C++ throws out of the query (a
    // malformed tile).
    /// `queryRenderedFeatures(const ScreenLineString&, const RenderedQueryOptions&)`.
    pub fn query_rendered_features(
        &self,
        geometry: &[mln_util::geo::ScreenCoordinate],
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        self.impl_ref().orchestrator.query_rendered_features(geometry, options)
    }

    /// `queryRenderedFeatures(const ScreenCoordinate&, const RenderedQueryOptions&)`.
    pub fn query_rendered_features_at_point(
        &self,
        point: &mln_util::geo::ScreenCoordinate,
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        self.impl_ref().orchestrator.query_rendered_features(&[*point], options)
    }

    /// `queryRenderedFeatures(const ScreenBox&, const RenderedQueryOptions&)`.
    pub fn query_rendered_features_in_box(
        &self,
        min: &mln_util::geo::ScreenCoordinate,
        max: &mln_util::geo::ScreenCoordinate,
        options: &crate::query::RenderedQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        use mln_util::geo::ScreenCoordinate;
        self.impl_ref().orchestrator.query_rendered_features(
            &[*min, ScreenCoordinate::new(max.x, min.y), *max, ScreenCoordinate::new(min.x, max.y), *min],
            options,
        )
    }

    /// `querySourceFeatures(sourceID, options)`.
    pub fn query_source_features(
        &self,
        source_id: &str,
        options: &crate::query::SourceQueryOptions,
    ) -> Result<Vec<mln_util::feature::Feature>, mln_util::error::Error> {
        self.impl_ref().orchestrator.query_source_features(source_id, options)
    }

    /// `queryFeatureExtensions(sourceID, feature, extension, extensionField,
    /// args)`: for a GeoJSON source with `cluster` on, the "supercluster"
    /// extension's "children", "leaves" or "expansion-zoom" of a cluster
    /// feature (one with a `cluster_id` property). Anything else is a null
    /// value. `Err` is the `std::runtime_error` C++ throws for a cluster ID
    /// the source doesn't have.
    pub fn query_feature_extensions(
        &self,
        source_id: &str,
        feature: &mln_util::feature::Feature,
        extension: &str,
        extension_field: &str,
        args: Option<&feature::PropertyMap>,
    ) -> Result<crate::query::FeatureExtensionValue, mln_util::error::Error> {
        self.impl_ref().orchestrator.query_feature_extensions(source_id, feature, extension, extension_field, args)
    }

    /// `getPlacedSymbolsData()`.
    #[must_use]
    pub fn get_placed_symbols_data(&self) -> Vec<crate::text::placement::PlacedSymbolData> {
        self.impl_ref().orchestrator.get_placed_symbols_data()
    }

    /// `dumpDebugLogs()`: logs every render source's tiles and the image
    /// manager's state.
    pub fn dump_debug_logs(&self) {
        self.impl_ref().orchestrator.dump_debug_logs();
    }

    // TODO(port): queryPointAnnotations, queryShapeAnnotations,
    // queryFeatureExtensions.

    // layers: feature state
    /// `setFeatureState(sourceID, sourceLayerID, featureID, state)`.
    pub fn set_feature_state(
        &mut self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
        state: &mln_style::expression::expression::FeatureState,
    ) {
        self.impl_mut().orchestrator.set_feature_state(source_id, source_layer_id, feature_id, state);
    }

    /// `getFeatureState(sourceID, sourceLayerID, featureID)`.
    #[must_use]
    pub fn get_feature_state(
        &self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: &str,
    ) -> mln_style::expression::expression::FeatureState {
        self.impl_ref().orchestrator.get_feature_state(source_id, source_layer_id, feature_id)
    }

    /// `removeFeatureState(sourceID, sourceLayerID, featureID, stateKey)`.
    pub fn remove_feature_state(
        &mut self,
        source_id: &str,
        source_layer_id: Option<&str>,
        feature_id: Option<&str>,
        state_key: Option<&str>,
    ) {
        self.impl_mut().orchestrator.remove_feature_state(source_id, source_layer_id, feature_id, state_key);
    }

    pub fn collect_placed_symbol_data(&mut self, enable: bool) {
        self.impl_mut().orchestrator.collect_placed_symbol_data(enable);
    }

    pub fn set_tile_cache_enabled(&mut self, enable: bool) {
        self.impl_mut().orchestrator.set_tile_cache_enabled(enable);
    }

    #[must_use]
    pub fn get_tile_cache_enabled(&self) -> bool {
        self.impl_ref().orchestrator.get_tile_cache_enabled()
    }

    /// Tells the hosts of the custom layers that the renderer's graphics
    /// objects are lost, so that they forget theirs, and lets go of them.
    /// Call it before the renderer goes when its device or target went first.
    pub fn lose_custom_layers(&mut self) {
        self.impl_mut().lose_custom_layers();
    }

    pub fn reduce_memory_use(&mut self) {
        let backend = self.impl_ref().backend.clone();
        let _guard = BackendScope::new(&*backend, ScopeType::Explicit);
        self.impl_mut().reduce_memory_use();
        self.impl_mut().orchestrator.reduce_memory_use();
    }

    pub fn clear_data(&mut self) {
        self.impl_mut().orchestrator.clear_data();
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // `gfx::BackendScope guard{impl->backend}; impl.reset();`
        if let Some(impl_) = self.impl_.take() {
            let backend = impl_.backend.clone();
            let _guard = BackendScope::new(&*backend, ScopeType::Explicit);
            drop(impl_);
        }
    }
}
