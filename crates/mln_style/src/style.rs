use std::sync::Arc;

use feature::Value;
use mln_util::error::Error;

use crate::expression::GlobalStateMap;
use crate::image;
use crate::layer::Layer;
use crate::light::Light;
use crate::map::camera::CameraOptions;
use crate::projection::Projection;
use crate::registry::StyleRegistry;
use crate::sky::Sky;
use crate::source::{self, Source};
use crate::source_list::SourceList;
use crate::sprite::Sprite;
pub(crate) use crate::style_impl::Impl;
use crate::transition_options::TransitionOptions;

/// Why a style failed to load.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub enum StyleLoadError {
    /// The style document isn't a valid style.
    Parse(String),
    /// The style's request failed, such as with a 404 or a missing file.
    Load(String),
}

impl StyleLoadError {
    /// What failed, for people.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            StyleLoadError::Parse(message) | StyleLoadError::Load(message) => message,
        }
    }
}

/// A parsed style document: the style, and the sources it names, whose
/// records the style holds.
pub struct Parsed {
    pub style: Style,
    pub sources: Vec<Box<dyn Source>>,
}

#[derive(Debug)]
pub struct Style {
    // Private implementation
    pub impl_: Box<Impl>,
}

impl Style {
    /// An empty style whose JSON, layers and filters parse against
    /// `registry`.
    #[must_use]
    pub fn new(registry: Arc<StyleRegistry>) -> Self {
        Style { impl_: Box::new(Impl::new(registry)) }
    }

    /// The style that the style document `json` describes, parsed against
    /// `registry`, with `fallback_font_faces` after the document's font
    /// faces, and the document's sources. Fails with why `json` isn't a
    /// style.
    pub fn from_json(
        registry: &Arc<StyleRegistry>,
        json: &str,
        fallback_font_faces: Option<&Arc<mln_text::glyph::FontFaces>>,
    ) -> Result<Parsed, Error> {
        let (style, sources) = Impl::from_json(registry, json, fallback_font_faces)?;
        Ok(Parsed { style: Style { impl_: Box::new(style) }, sources })
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<StyleRegistry> {
        self.impl_.registry()
    }

    /// The document's sprites.
    #[must_use]
    pub fn get_sprites(&self) -> &[Sprite] {
        self.impl_.get_sprites()
    }

    /// Marks the style not loaded while a URL load fetches the document
    /// that replaces it.
    pub fn begin_url_load(&mut self) {
        self.impl_.begin_url_load();
    }

    /// Whether the style has been edited since its document parsed.
    #[must_use]
    pub fn is_mutated(&self) -> bool {
        self.impl_.mutated
    }

    #[must_use]
    pub fn get_json(&self) -> String {
        self.impl_.get_json()
    }

    /// Whether a style document parsed, and no URL load began since.
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.impl_.loaded
    }

    // Defaults
    #[must_use]
    pub fn get_name(&self) -> String {
        self.impl_.get_name()
    }

    #[must_use]
    pub fn get_default_camera(&self) -> CameraOptions {
        self.impl_.get_default_camera()
    }

    // TransitionOptions
    #[must_use]
    pub fn get_transition_options(&self) -> TransitionOptions {
        self.impl_.get_transition_options()
    }

    pub fn set_transition_options(&mut self, options: &TransitionOptions) {
        self.impl_.mutated = true;
        self.impl_.set_transition_options(options);
    }

    // Light
    #[must_use]
    pub fn get_light(&self) -> &Arc<Light> {
        self.impl_.get_light()
    }

    pub fn set_light(&mut self, light: Light) {
        self.impl_.set_light(light);
    }

    // Sky
    /// Sets the sky; `None` removes it.
    pub(crate) fn set_sky(&mut self, sky: Option<Sky>) {
        self.impl_.mutated = true;
        self.impl_.set_sky(sky);
    }

    #[must_use]
    pub fn get_sky(&self) -> Option<&Arc<Sky>> {
        self.impl_.get_sky()
    }

    // Projection
    #[must_use]
    pub fn get_projection(&self) -> &Arc<Projection> {
        self.impl_.get_projection()
    }

    pub fn set_projection(&mut self, projection: Projection) {
        self.impl_.mutated = true;
        self.impl_.set_projection(projection);
    }

    // Global state, used by the "global-state" expression.
    #[must_use]
    pub(crate) fn get_global_state(&self) -> GlobalStateMap {
        self.impl_.get_global_state()
    }

    /// Fails with [`ErrorKind::InvalidState`](mln_util::ErrorKind) if the
    /// style has not finished loading.
    pub fn set_global_state_property(&mut self, name: &str, value: &Value) -> Result<(), Error> {
        if !self.impl_.loaded {
            return Err(Error::invalid_state("Style is not done loading."));
        }

        self.impl_.mutated = true;
        self.impl_.set_global_state_property(name, value);
        Ok(())
    }

    // Images
    #[must_use]
    pub fn get_image(&self, name: &str) -> Option<Arc<image::Impl>> {
        self.impl_.get_image(name)
    }

    pub fn add_image(&mut self, image: Arc<image::Impl>) {
        self.impl_.mutated = true;
        self.impl_.add_image(image);
    }

    pub fn remove_image(&mut self, name: &str) {
        self.impl_.mutated = true;
        self.impl_.remove_image(name);
    }

    /// Adds a sprite's images, each replacing the image with its id. Unlike
    /// [`add_image`](Self::add_image), it doesn't count as an edit.
    pub fn add_sprite_images(&mut self, images: Vec<Arc<image::Impl>>) {
        self.impl_.add_images(images);
    }

    // Sources

    /// The records of the style's sources, in the order they were added.
    #[must_use]
    pub fn get_sources(&self) -> &Arc<SourceList> {
        self.impl_.get_sources()
    }

    /// The record of the source `id`.
    #[must_use]
    pub fn get_source(&self, id: &str) -> Option<&Arc<dyn source::Impl>> {
        self.impl_.get_sources().get(id)
    }

    /// Records `source` as the source with its id, replacing that source's
    /// record in place or adding it last, or removes the record of the
    /// source `id` for `None`. The map, which holds the sources' handles,
    /// keeps the records current. Counts as an edit when the records change.
    pub fn set_source_impl(&mut self, id: &str, source: Option<Arc<dyn source::Impl>>) {
        if self.impl_.set_source_impl(id, source) {
            self.impl_.mutated = true;
        }
    }

    /// As [`set_source_impl`](Self::set_source_impl), for a change that a
    /// source made itself, which doesn't count as an edit. Returns whether
    /// the records changed.
    pub fn refresh_source_impl(&mut self, id: &str, source: Arc<dyn source::Impl>) -> bool {
        self.impl_.set_source_impl(id, Some(source))
    }

    // Layers
    /// The layers, bottom first.
    pub fn get_layers(&self) -> impl DoubleEndedIterator<Item = &Layer> + ExactSizeIterator {
        self.impl_.get_layer_list().iter()
    }

    /// The place of the layer `id` in drawing order, bottom first.
    #[must_use]
    pub fn layer_index(&self, layer_id: &str) -> Option<usize> {
        self.impl_.get_layer_list().index_of(layer_id)
    }

    #[must_use]
    pub fn get_layer(&self, layer_id: &str) -> Option<&Layer> {
        self.impl_.get_layer(layer_id)
    }

    /// Calls `f` with the layer `layer_id` to change it, and returns what `f`
    /// returns, or `None` when the style has no such layer. Marks the style
    /// mutated. A change to the layer replaces it in the next version of the
    /// layer list; a call that changes nothing leaves the list as it was.
    pub fn update_layer<R>(&mut self, layer_id: &str, f: impl FnOnce(&mut Layer) -> R) -> Option<R> {
        self.impl_.mutated = true;
        self.impl_.update_layer(layer_id, f)
    }

    /// Fails with [`ErrorKind::AlreadyExists`](mln_util::ErrorKind) if the
    /// style has a layer with the same ID, and with
    /// [`ErrorKind::InvalidArgument`](mln_util::ErrorKind) if the layer's
    /// type doesn't fit its source.
    pub fn add_layer(&mut self, layer: Layer, before_layer_id: Option<&str>) -> Result<(), Error> {
        self.impl_.mutated = true;
        self.impl_.add_layer(layer, before_layer_id)
    }

    pub fn remove_layer(&mut self, id: &str) -> Option<Layer> {
        self.impl_.mutated = true;
        self.impl_.remove_layer(id)
    }
}
