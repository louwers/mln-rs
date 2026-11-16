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
use crate::source::Source;
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

#[derive(Debug)]
pub struct Style {
    // Private implementation
    pub impl_: Box<Impl>,
}

impl Style {
    /// A style without a file source, on `run_loop`, whose JSON, layers and
    /// filters parse against `registry` (see `Impl::new`).
    #[must_use]
    pub fn new(registry: Arc<StyleRegistry>, run_loop: &mln_actor::RunLoop) -> Self {
        Style { impl_: Box::new(Impl::new(registry, run_loop)) }
    }

    /// A style that parses against `registry` and whose sources load from
    /// `file_source`, on `run_loop` (see `Impl::with_file_source`).
    #[must_use]
    pub fn with_file_source(
        registry: Arc<StyleRegistry>,
        file_source: Option<Arc<dyn mln_storage::FileSource>>,
        run_loop: &mln_actor::RunLoop,
    ) -> Self {
        Style { impl_: Box::new(Impl::with_file_source(registry, file_source, run_loop)) }
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<StyleRegistry> {
        self.impl_.registry()
    }

    /// Replaces the style with the one the style document `json`
    /// describes, with `fallback_font_faces` after the document's font
    /// faces. Returns the document's sprites, for the caller to load, or
    /// why `json` isn't a style, which leaves the style as it was.
    pub fn parse_json(
        &mut self,
        json: &str,
        fallback_font_faces: Option<&Arc<mln_text::glyph::FontFaces>>,
    ) -> Result<Vec<Sprite>, Error> {
        self.impl_.parse(json, fallback_font_faces)
    }

    /// Marks the style not loaded until the next [`parse_json`](Self::parse_json),
    /// while a URL load fetches its document.
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
    #[must_use]
    pub fn get_sources(&self) -> Vec<&dyn Source> {
        self.impl_.get_sources()
    }

    #[must_use]
    pub fn get_source(&self, id: &str) -> Option<&dyn Source> {
        self.impl_.get_source(id)
    }

    /// Fails with [`ErrorKind::AlreadyExists`](mln_util::ErrorKind) if the
    /// style has a source with the same ID.
    pub fn add_source(&mut self, source: Box<dyn Source>) -> Result<(), Error> {
        self.impl_.mutated = true;
        self.impl_.add_source(source)
    }

    pub fn remove_source(&mut self, source_id: &str) -> Option<Box<dyn Source>> {
        self.impl_.mutated = true;
        self.impl_.remove_source(source_id)
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
