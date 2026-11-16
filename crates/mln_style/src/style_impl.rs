// Deviations (structural; see also layer.rs):
// - The layers, sources, light, sky and projection are values behind
//   `Arc`s, which the style replaces when they change. `update_layer` edits
//   a copy of a layer and keeps it if the layer changed.
// - The observer is an `Rc<dyn Observer>`.

use std::rc::Rc;
use std::sync::Arc;

use feature::Value;
use mln_util::error::Error;
use mln_util::logging::{Event, Log};

use crate::expression::GlobalStateMap;
use crate::image;
use crate::layer::Layer;
use crate::layer_list::LayerList;
use crate::light::Light;
use crate::map::camera::CameraOptions;
use crate::observer::{NullObserver, Observer};
use crate::parser::Parser;
use crate::projection::Projection;
use crate::registry::StyleRegistry;
use crate::sky::Sky;
use crate::source::{self, Source, layer_kind};
use crate::source_list::SourceList;
use crate::sprite::Sprite;
use crate::transition_options::TransitionOptions;

pub(crate) type ImageImpls = Vec<Arc<image::Impl>>;

pub struct Impl {
    json: String,

    glyph_url: String,
    /// The document's font faces, followed by the fallback faces given to
    /// the parse.
    font_faces: Option<Arc<mln_text::glyph::FontFaces>>,
    /// The records of the sources whose handles the map holds.
    sources: Arc<SourceList>,
    layers: Arc<LayerList>,
    transition_options: TransitionOptions,
    light: Arc<Light>,
    sky: Option<Arc<Sky>>,
    projection: Arc<Projection>,
    images: Arc<ImageImpls>,
    observer: Option<Rc<dyn Observer>>,

    // Global state for the "global-state" expression. The map itself is
    // immutable and replaced wholesale on change so that it can be safely
    // shared across threads.
    global_state: Arc<GlobalStateMap>,
    global_state_defaults: GlobalStateMap,

    // Defaults
    name: String,
    default_camera: CameraOptions,
    /// The document's sprites.
    sprites: Vec<Sprite>,

    registry: Arc<StyleRegistry>,

    pub(crate) mutated: bool,
    pub(crate) loaded: bool,
}

impl std::fmt::Debug for Impl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("style::Impl")
            .field("name", &self.name)
            .field("layers", &self.layers)
            .field("loaded", &self.loaded)
            .finish_non_exhaustive()
    }
}

impl Impl {
    /// An empty style that parses against `registry`.
    #[must_use]
    pub(crate) fn new(registry: Arc<StyleRegistry>) -> Self {
        Impl {
            json: String::new(),
            glyph_url: String::new(),
            font_faces: None,
            sources: Arc::default(),
            layers: Arc::default(),
            transition_options: TransitionOptions::default(),
            light: Arc::default(),
            sky: None,
            projection: Arc::default(),
            images: Arc::new(Vec::new()),
            observer: None,
            global_state: Arc::new(GlobalStateMap::new()),
            global_state_defaults: GlobalStateMap::new(),
            name: String::new(),
            default_camera: CameraOptions::default(),
            sprites: Vec::new(),
            registry,
            mutated: false,
            loaded: false,
        }
    }

    fn observer(&self) -> Rc<dyn Observer> {
        match &self.observer {
            Some(observer) => Rc::clone(observer),
            None => Rc::new(NullObserver),
        }
    }

    /// Marks the style not loaded while a URL load fetches its document.
    pub(crate) fn begin_url_load(&mut self) {
        self.loaded = false;
    }

    /// The style that `json` describes, parsed against `registry`, with
    /// `fallback_font_faces` after the document's font faces, and the
    /// document's sources, whose records it holds. Fails with why `json`
    /// isn't a style.
    pub(crate) fn from_json(
        registry: &Arc<StyleRegistry>,
        json: &str,
        fallback_font_faces: Option<&Arc<mln_text::glyph::FontFaces>>,
    ) -> Result<(Self, Vec<Box<dyn Source>>), Error> {
        let mut parser = Parser::new(registry);
        parser.parse(json)?;

        let mut style = Impl::new(Arc::clone(registry));
        style.json = json.to_owned();
        style.transition_options = parser.transition;
        style.global_state_defaults = parser.global_state_defaults.clone();
        style.global_state = Arc::new(style.global_state_defaults.clone());

        let mut sources: Vec<Box<dyn Source>> = Vec::new();
        let mut records = SourceList::default();
        for source in std::mem::take(&mut parser.sources) {
            // A parsed style cannot contain duplicate source ids (they are
            // object keys).
            if records.get(&source.get_id()).is_some() {
                Log::error(Event::ParseStyle, format!("Source {} already exists", source.get_id()));
                continue;
            }
            records.set(source.base_impl());
            sources.push(source);
        }
        style.sources = Arc::new(records);

        // The parser drops duplicate layer ids, so the layers go in at once
        // without `add_layer`'s lookup. A layer incompatible with its source
        // is logged and skipped.
        let global_state = Arc::clone(&style.global_state);
        let mut layers = LayerList::default();
        layers.extend(std::mem::take(&mut parser.layers).into_iter().filter_map(|mut layer| {
            if let Err(error) = check_layer_source(&style.sources, &layer) {
                Log::error(Event::ParseStyle, error.to_string());
                return None;
            }
            layer.reevaluate_visibility(&global_state);
            Some(layer)
        }));
        style.layers = Arc::new(layers);

        style.name = parser.name.clone();
        style.default_camera.center = Some(parser.lat_lng);
        style.default_camera.center_altitude = Some(parser.center_altitude);
        style.default_camera.zoom = Some(parser.zoom);
        style.default_camera.bearing = Some(parser.bearing);
        style.default_camera.pitch = Some(parser.pitch);
        style.default_camera.roll = Some(parser.roll);

        style.light = Arc::new(std::mem::take(&mut parser.light));
        style.sky = parser.sky.take().map(Arc::new);
        style.projection = Arc::new(std::mem::take(&mut parser.projection));

        style.sprites = std::mem::take(&mut parser.sprites);
        style.glyph_url = std::mem::take(&mut parser.glyph_url);
        style.font_faces = parser.font_faces.take();
        if let Some(fallback) = fallback_font_faces.filter(|faces| !faces.is_empty()) {
            // The style's own faces come first, so they win where both cover a font.
            let mut merged = style.font_faces.as_deref().cloned().unwrap_or_default();
            merged.extend(fallback.iter().cloned());
            style.font_faces = Some(Arc::new(merged));
        }
        style.loaded = true;
        Ok((style, sources))
    }

    /// Replaces the style with the one `json` describes, keeping its
    /// observer; a document that doesn't parse leaves the style as it was.
    #[cfg(test)]
    pub(crate) fn load_json(&mut self, json: &str) {
        if let Ok((mut style, _sources)) = Self::from_json(&Arc::clone(&self.registry), json, None) {
            style.observer = self.observer.take();
            *self = style;
        }
    }

    /// The document's sprites.
    pub(crate) fn get_sprites(&self) -> &[Sprite] {
        &self.sprites
    }

    pub(crate) fn registry(&self) -> &Arc<StyleRegistry> {
        &self.registry
    }

    pub(crate) fn get_json(&self) -> String {
        self.json.clone()
    }

    pub fn set_observer(&mut self, observer: Option<Rc<dyn Observer>>) {
        self.observer = observer;
    }

    pub(crate) fn set_transition_options(&mut self, options: &TransitionOptions) {
        if self.transition_options.duration == options.duration
            && self.transition_options.delay == options.delay
            && self.transition_options.enable_placement_transitions == options.enable_placement_transitions
        {
            return;
        }
        self.transition_options = *options;
        self.observer().on_update();
    }

    #[must_use]
    pub fn get_transition_options(&self) -> TransitionOptions {
        self.transition_options
    }

    /// Set a global state property. A null value resets the property to the
    /// default defined in the style's root "state" property (or null).
    pub(crate) fn set_global_state_property(&mut self, property: &str, value: &Value) {
        let mut new_value = value;
        if matches!(value, Value::Null)
            && let Some(default) = self.global_state_defaults.get(property)
        {
            new_value = default;
        }

        let unchanged = match self.global_state.get(property) {
            Some(current) => state_values_equal(current, new_value),
            None => false,
        };
        if unchanged {
            return;
        }

        let mut new_state = (*self.global_state).clone();
        new_state.insert(property.to_owned(), new_value.clone());
        self.global_state = Arc::new(new_state);
        self.reevaluate_layer_visibilities();
        self.observer().on_update();
    }

    /// Replaces the defaults that the style's root `state` gives, and sets
    /// every global state property to its new default, as a style that
    /// loads with this `state` would.
    pub(crate) fn set_global_state_defaults(&mut self, defaults: GlobalStateMap) {
        self.global_state = Arc::new(defaults.clone());
        self.global_state_defaults = defaults;
        self.reevaluate_layer_visibilities();
        self.observer().on_update();
    }

    /// The defaults that the style's root `state` gives.
    pub(crate) fn get_global_state_defaults(&self) -> &GlobalStateMap {
        &self.global_state_defaults
    }

    /// Re-evaluate the visibility expressions of all layers against the
    /// current global state.
    pub(crate) fn reevaluate_layer_visibilities(&mut self) {
        let mut changed = Vec::new();
        for (index, layer) in self.layers.iter().enumerate() {
            let mut copy = layer.clone();
            copy.reevaluate_visibility(&self.global_state);
            if !Arc::ptr_eq(layer.style(), copy.style()) {
                changed.push((index, copy));
            }
        }
        if changed.is_empty() {
            return;
        }
        let observer = self.observer();
        let layers = Arc::make_mut(&mut self.layers);
        for (index, layer) in changed {
            layers.replace(index, layer);
            // An update for each layer that changed.
            observer.on_update();
        }
    }

    pub(crate) fn get_global_state(&self) -> GlobalStateMap {
        (*self.global_state).clone()
    }

    #[must_use]
    pub fn get_global_state_shared(&self) -> Arc<GlobalStateMap> {
        Arc::clone(&self.global_state)
    }

    // Sources

    /// The records of the sources, in the order they were added.
    pub(crate) fn get_sources(&self) -> &Arc<SourceList> {
        &self.sources
    }

    /// Records `source` as the source with its id, replacing the record of
    /// that source in place or adding it last, or removes the record of the
    /// source `id` for `None`. Returns whether the records changed.
    pub(crate) fn set_source_impl(&mut self, id: &str, source: Option<Arc<dyn source::Impl>>) -> bool {
        if let Some(source) = source {
            if self.sources.get(id).is_some_and(|current| Arc::ptr_eq(current, &source)) {
                return false;
            }
            Arc::make_mut(&mut self.sources).set(source);
            true
        } else {
            if self.sources.get(id).is_none() {
                return false;
            }
            Arc::make_mut(&mut self.sources).remove(id);
            true
        }
    }

    // Layers

    /// The layers, in drawing order.
    #[must_use]
    pub fn get_layer_list(&self) -> &Arc<LayerList> {
        &self.layers
    }

    pub(crate) fn get_layer(&self, id: &str) -> Option<&Layer> {
        self.layers.get(id)
    }

    /// Calls `f` with a copy of the layer `id`, and keeps the copy if `f`
    /// changed the layer's style data: the style re-evaluates its visibility
    /// and publishes an update. `None` when the style has no layer `id`.
    pub(crate) fn update_layer<R>(&mut self, id: &str, f: impl FnOnce(&mut Layer) -> R) -> Option<R> {
        let (index, layer) = self.layers.get_full(id)?;
        let mut copy = layer.clone();
        let result = f(&mut copy);
        if !Arc::ptr_eq(layer.style(), copy.style()) {
            copy.reevaluate_visibility(&self.global_state);
            Arc::make_mut(&mut self.layers).replace(index, copy);
            self.observer().on_update();
        }
        Some(result)
    }

    /// Adds a layer before the layer `before`, or on top. See
    /// [`Style::add_layer`](crate::Style::add_layer).
    pub(crate) fn add_layer(&mut self, mut layer: Layer, before: Option<&str>) -> Result<(), Error> {
        check_layer_source(&self.sources, &layer)?;
        if self.layers.get(layer.get_id()).is_some() {
            return Err(Error::already_exists(format!("Layer {} already exists", layer.get_id())));
        }
        // The global state is not available while the layer itself is parsed.
        layer.reevaluate_visibility(&self.global_state);
        let inserted = Arc::make_mut(&mut self.layers).insert_before(layer, before);
        debug_assert!(inserted.is_ok());
        self.observer().on_update();
        Ok(())
    }

    /// Moves the layer `id` below the layer `before`, or above every layer,
    /// as one change. Returns false when the style has no layer `id`.
    pub(crate) fn move_layer(&mut self, id: &str, before: Option<&str>) -> bool {
        if self.layers.get(id).is_none() {
            return false;
        }
        Arc::make_mut(&mut self.layers).move_before(id, before);
        self.observer().on_update();
        true
    }

    pub(crate) fn remove_layer(&mut self, id: &str) -> Option<Layer> {
        self.layers.get(id)?;
        let layer = Arc::make_mut(&mut self.layers).remove(id);

        if layer.is_some() {
            self.observer().on_update();
        }

        layer
    }

    // Light

    pub(crate) fn set_light(&mut self, light: Light) {
        self.light = Arc::new(light);
        self.on_light_changed();
    }

    #[must_use]
    pub fn get_light(&self) -> &Arc<Light> {
        &self.light
    }

    /// Sets a property of the light (see [`Light::set_property`]); the light
    /// is replaced only if the value changes it.
    pub(crate) fn set_light_property(
        &mut self,
        name: &str,
        value: &mln_conversion::Convertible<'_>,
        functions: &Arc<crate::expression::Functions>,
    ) -> Option<mln_conversion::Error> {
        let before = Arc::clone(&self.light);
        let error = Light::set_shared_property(&mut self.light, name, value, functions);
        if !Arc::ptr_eq(&before, &self.light) {
            self.on_light_changed();
        }
        error
    }

    // Sky

    /// Sets the sky; `None` removes it.
    pub(crate) fn set_sky(&mut self, sky: Option<Sky>) {
        self.sky = sky.map(Arc::new);
        self.observer().on_update();
    }

    #[must_use]
    pub fn get_sky(&self) -> Option<&Arc<Sky>> {
        self.sky.as_ref()
    }

    // Projection

    pub(crate) fn set_projection(&mut self, projection: Projection) {
        self.projection = Arc::new(projection);
        self.on_projection_changed();
    }

    #[must_use]
    pub fn get_projection(&self) -> &Arc<Projection> {
        &self.projection
    }

    pub(crate) fn get_name(&self) -> String {
        self.name.clone()
    }

    pub(crate) fn get_default_camera(&self) -> CameraOptions {
        self.default_camera
    }

    // Images

    pub(crate) fn get_image(&self, id: &str) -> Option<Arc<image::Impl>> {
        self.images.iter().find(|image| image.id == id).cloned()
    }

    /// Adds an image, replacing the image with the same id.
    pub(crate) fn add_image(&mut self, image: Arc<image::Impl>) {
        let mut new_images = (*self.images).clone();
        let it = new_images.partition_point(|a| a.id < image.id);
        if it < new_images.len() && new_images[it].id == image.id {
            // We permit using addImage to update.
            new_images[it] = image;
        } else {
            new_images.insert(it, image);
        }
        self.images = Arc::new(new_images);
        self.observer().on_update();
    }

    /// Adds a sprite's images, each replacing the image with its id.
    pub(crate) fn add_images(&mut self, images: Vec<Arc<image::Impl>>) {
        let mut new_images = (*self.images).clone();
        let mut remaining = Vec::new();
        for image in images {
            let first = new_images.partition_point(|a| a.id < image.id);
            if first < new_images.len() && image.id == new_images[first].id {
                new_images[first] = image;
            } else {
                remaining.push(image);
            }
        }
        new_images.extend(remaining);
        new_images.sort_by(|a, b| a.id.cmp(&b.id));
        self.images = Arc::new(new_images);
    }

    pub(crate) fn remove_image(&mut self, id: &str) {
        let mut new_images = (*self.images).clone();
        let Some(found) = new_images.iter().position(|image| image.id == id) else {
            Log::warning(Event::General, format!("Image '{id}' is not present in style, cannot remove"));
            return;
        };
        new_images.remove(found);
        self.images = Arc::new(new_images);
        self.observer().on_update();
    }

    /// symbols: `getFontFaces()`.
    #[must_use]
    pub fn get_font_faces(&self) -> Option<Arc<mln_text::glyph::FontFaces>> {
        self.font_faces.clone()
    }

    #[must_use]
    pub fn get_glyph_url(&self) -> &str {
        &self.glyph_url
    }

    #[must_use]
    pub fn get_image_impls(&self) -> Arc<ImageImpls> {
        Arc::clone(&self.images)
    }

    // LightObserver implementation.

    pub(crate) fn on_light_changed(&mut self) {
        self.observer().on_update();
    }

    // SkyObserver implementation.

    // ProjectionObserver implementation.

    pub(crate) fn on_projection_changed(&mut self) {
        let observer = self.observer();
        observer.on_projection_changed();
        observer.on_update();
    }
}

/// Fails when `layer`'s source is in `sources` and doesn't feed layers of
/// the layer's type.
fn check_layer_source(sources: &SourceList, layer: &Layer) -> Result<(), Error> {
    let impl_ = layer.style();
    if let Some(source) = sources.get(&impl_.base().source)
        && layer_kind(source.type_) != impl_.info().source
    {
        let message = format!("Layer '{}' is not compatible with source '{}'", impl_.base().id, impl_.base().source);
        return Err(Error::invalid_argument(message));
    }
    Ok(())
}

/// Equality on state values that, unlike the exact-type variant equality,
/// treats numerically equal numbers of different arithmetic types as equal
/// (e.g. a style default parsed as an integer vs. a runtime-set double).
fn state_values_equal(a: &Value, b: &Value) -> bool {
    fn to_double(v: &Value) -> Option<f64> {
        match v {
            Value::Uint(u) => Some(*u as f64),
            Value::Int(i) => Some(*i as f64),
            Value::Double(d) => Some(*d),
            _ => None,
        }
    }
    if let Some(a) = to_double(a) {
        return match to_double(b) {
            Some(b) => a == b,
            None => false,
        };
    }
    match (a, b) {
        (Value::Array(array_a), Value::Array(array_b)) => {
            array_a.len() == array_b.len() && array_a.iter().zip(array_b.iter()).all(|(x, y)| state_values_equal(x, y))
        }
        (Value::Object(object_a), Value::Object(object_b)) => {
            if object_a.len() != object_b.len() {
                return false;
            }
            object_a.iter().all(|(key, value)| match object_b.get(key) {
                Some(other) => state_values_equal(value, other),
                None => false,
            })
        }
        _ => a == b,
    }
}
