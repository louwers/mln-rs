// Deviations (structural; see also layer.rs):
// - The layers, light, sky and projection are values behind `Arc`s, which
//   the style replaces when they change. `update_layer` edits a copy of a
//   layer and keeps it if the layer changed.
// - The observer is an `Rc<dyn Observer>`, shared with the sources'
//   observer (`StyleSourceObserver`).
//
// - Sources notify from setters and from asynchronous request callbacks
//   (see source.rs), so the observer handed to them is a
//   `StyleSourceObserver` sharing the style's observer and the file source.
//   `sources.update(source)` happens when the impls are read
//   (`get_source_impls`), which picks up every source whose impl changed.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use feature::Value;
use mln_actor::RunLoop;
use mln_storage::FileSource;
use mln_util::error::Error;
use mln_util::logging::{Event, Log};

use crate::collection::{CollectionWithPersistentOrder, ImmutableVector};
use crate::expression::GlobalStateMap;
use crate::image;
use crate::layer::Layer;
use crate::layer_list::LayerList;
use crate::light::Light;
use crate::map::camera::CameraOptions;
use crate::observer::{NullObserver, Observer, SourceObserver};
use crate::parser::Parser;
use crate::projection::Projection;
use crate::registry::StyleRegistry;
use crate::sky::Sky;
use crate::source::{self, Source};
use crate::sprite::Sprite;
use crate::transition_options::TransitionOptions;

pub(crate) type ImageImpls = Vec<Arc<image::Impl>>;

/// The style's observer, shared with its sources' observer.
type ObserverSlot = Rc<RefCell<Option<Rc<dyn Observer>>>>;

fn slot_observer(slot: &ObserverSlot) -> Rc<dyn Observer> {
    match &*slot.borrow() {
        Some(observer) => Rc::clone(observer),
        None => Rc::new(NullObserver),
    }
}

/// The `SourceObserver` half of `Style::Impl`.
struct StyleSourceObserver {
    observer: std::rc::Weak<RefCell<Option<Rc<dyn Observer>>>>,
    file_source: Option<Arc<dyn FileSource>>,
    run_loop: RunLoop,
}

impl StyleSourceObserver {
    fn observer(&self) -> Option<Rc<dyn Observer>> {
        self.observer.upgrade().map(|slot| slot_observer(&slot))
    }
}

impl SourceObserver for StyleSourceObserver {
    fn on_source_loaded(&self, source: &dyn Source) {
        // sources.update(source): see get_source_impls.
        if let Some(observer) = self.observer() {
            observer.on_source_loaded(source);
            observer.on_update();
        }
    }

    fn on_source_changed(&self, source: &dyn Source) {
        // sources.update(source): see get_source_impls.
        if let Some(observer) = self.observer() {
            observer.on_source_changed(source);
            observer.on_update();
        }
    }

    fn on_source_error(&self, source: &dyn Source, error: Error) {
        if let Some(observer) = self.observer() {
            observer.on_source_error(source, error);
        }
    }

    fn on_source_description_changed(&self, source: &dyn Source) {
        // sources.update(source): see get_source_impls.
        if let Some(observer) = self.observer() {
            observer.on_source_description_changed(source);
        }
        if !source.loaded()
            && let Some(file_source) = &self.file_source
        {
            source.load_description(&**file_source, &self.run_loop);
        }
    }
}

/// The collection's source impls, and the source impls that refresh them
/// with the changes made through the sources.
type SourceImplsCache = (ImmutableVector<dyn source::Impl>, ImmutableVector<dyn source::Impl>);

pub struct Impl {
    file_source: Option<Arc<dyn FileSource>>,

    json: String,

    glyph_url: String,
    /// The document's font faces, followed by the fallback faces given to
    /// the parse.
    font_faces: Option<Arc<mln_text::glyph::FontFaces>>,
    sources: CollectionWithPersistentOrder<dyn Source>,
    /// The source impls with the changes made through the sources since the
    /// collection's last update (see `get_source_impls`), keyed by the
    /// collection's impls they refresh.
    source_impls_cache: RefCell<Option<SourceImplsCache>>,
    source_observer: Rc<dyn SourceObserver>,
    /// The loop that the sources' requests answer on.
    run_loop: RunLoop,
    layers: Arc<LayerList>,
    transition_options: TransitionOptions,
    light: Arc<Light>,
    sky: Option<Arc<Sky>>,
    projection: Arc<Projection>,
    images: Arc<ImageImpls>,
    observer: ObserverSlot,

    // Global state for the "global-state" expression. The map itself is
    // immutable and replaced wholesale on change so that it can be safely
    // shared across threads.
    global_state: Arc<GlobalStateMap>,
    global_state_defaults: GlobalStateMap,

    // Defaults
    name: String,
    default_camera: CameraOptions,

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
    /// A style without a file source, on `run_loop`, that parses against
    /// `registry`.
    #[must_use]
    pub(crate) fn new(registry: Arc<StyleRegistry>, run_loop: &RunLoop) -> Self {
        Self::with_file_source(registry, None, run_loop)
    }

    /// A style that parses against `registry` and whose sources load their
    /// descriptions from `file_source`, answering on `run_loop`.
    #[must_use]
    pub(crate) fn with_file_source(
        registry: Arc<StyleRegistry>,
        file_source: Option<Arc<dyn FileSource>>,
        run_loop: &RunLoop,
    ) -> Self {
        let observer: ObserverSlot = Rc::new(RefCell::new(None));
        let source_observer: Rc<dyn SourceObserver> = Rc::new(StyleSourceObserver {
            observer: Rc::downgrade(&observer),
            file_source: file_source.clone(),
            run_loop: run_loop.clone(),
        });
        Impl {
            file_source,
            json: String::new(),
            glyph_url: String::new(),
            font_faces: None,
            sources: CollectionWithPersistentOrder::default(),
            source_impls_cache: RefCell::new(None),
            source_observer,
            run_loop: run_loop.clone(),
            layers: Arc::default(),
            transition_options: TransitionOptions::default(),
            light: Arc::default(),
            sky: None,
            projection: Arc::default(),
            images: Arc::new(Vec::new()),
            observer,
            global_state: Arc::new(GlobalStateMap::new()),
            global_state_defaults: GlobalStateMap::new(),
            name: String::new(),
            default_camera: CameraOptions::default(),
            registry,
            mutated: false,
            loaded: false,
        }
    }

    fn observer(&self) -> Rc<dyn Observer> {
        slot_observer(&self.observer)
    }

    /// Marks the style not loaded while a URL load fetches its document.
    pub(crate) fn begin_url_load(&mut self) {
        self.loaded = false;
    }

    /// Replaces the style with the one `json` describes, with
    /// `fallback_font_faces` after the document's font faces. Returns the
    /// document's sprites, or why `json` isn't a style, which leaves the
    /// style as it was. The style's images go: its sprites bring them again.
    pub(crate) fn parse(
        &mut self,
        json: &str,
        fallback_font_faces: Option<&Arc<mln_text::glyph::FontFaces>>,
    ) -> Result<Vec<Sprite>, Error> {
        let mut parser = Parser::new(&self.registry);
        parser.parse(json)?;

        self.mutated = false;
        self.loaded = false;
        self.json = json.to_owned();

        self.sources.clear();
        self.layers = Arc::default();
        self.images = Arc::new(Vec::new());

        self.transition_options = parser.transition;

        self.global_state_defaults = parser.global_state_defaults.clone();
        self.global_state = Arc::new(self.global_state_defaults.clone());

        for source in std::mem::take(&mut parser.sources) {
            // A parsed style cannot contain duplicate source ids (they are
            // object keys).
            if let Err(error) = self.add_source_unnotified(source) {
                Log::error(Event::ParseStyle, error.to_string());
            }
        }

        // The parser drops duplicate layer ids, so the layers go in at once
        // without `add_layer`'s lookup. A layer incompatible with its source
        // is logged and skipped.
        let global_state = Arc::clone(&self.global_state);
        let sources = &self.sources;
        let mut layers = LayerList::default();
        layers.extend(std::mem::take(&mut parser.layers).into_iter().filter_map(|mut layer| {
            if let Err(error) = check_layer_source(sources, &layer) {
                Log::error(Event::ParseStyle, error.to_string());
                return None;
            }
            layer.reevaluate_visibility(&global_state);
            Some(layer)
        }));
        self.layers = Arc::new(layers);

        self.name = parser.name.clone();
        self.default_camera.center = Some(parser.lat_lng);
        self.default_camera.center_altitude = Some(parser.center_altitude);
        self.default_camera.zoom = Some(parser.zoom);
        self.default_camera.bearing = Some(parser.bearing);
        self.default_camera.pitch = Some(parser.pitch);
        self.default_camera.roll = Some(parser.roll);

        self.light = Arc::new(std::mem::take(&mut parser.light));
        self.sky = parser.sky.take().map(Arc::new);
        self.projection = Arc::new(std::mem::take(&mut parser.projection));

        self.glyph_url = std::mem::take(&mut parser.glyph_url);
        self.font_faces = parser.font_faces.take();
        if let Some(fallback) = fallback_font_faces.filter(|faces| !faces.is_empty()) {
            // The style's own faces come first, so they win where both cover a font.
            let mut merged = self.font_faces.as_deref().cloned().unwrap_or_default();
            merged.extend(fallback.iter().cloned());
            self.font_faces = Some(Arc::new(merged));
        }
        self.loaded = true;
        Ok(std::mem::take(&mut parser.sprites))
    }

    pub(crate) fn registry(&self) -> &Arc<StyleRegistry> {
        &self.registry
    }

    pub(crate) fn get_json(&self) -> String {
        self.json.clone()
    }

    pub fn set_observer(&mut self, observer: Option<Rc<dyn Observer>>) {
        *self.observer.borrow_mut() = observer;
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

    pub fn get_global_state_shared(&self) -> Arc<GlobalStateMap> {
        Arc::clone(&self.global_state)
    }

    // Sources

    pub(crate) fn get_sources(&self) -> Vec<&dyn Source> {
        self.sources.get_wrappers()
    }

    pub(crate) fn get_source(&self, id: &str) -> Option<&dyn Source> {
        self.sources.get(id)
    }

    /// See [`Style::add_source`](crate::Style::add_source).
    pub(crate) fn add_source(&mut self, source: Box<dyn Source>) -> Result<(), Error> {
        self.add_source_unnotified(source)?;
        self.observer().on_update();
        Ok(())
    }

    fn add_source_unnotified(&mut self, source: Box<dyn Source>) -> Result<(), Error> {
        if self.sources.get(&source.get_id()).is_some() {
            return Err(Error::already_exists(format!("Source {} already exists", source.get_id())));
        }

        source.set_observer(Some(Rc::clone(&self.source_observer)));
        let item = self.sources.add(source);
        if let Some(file_source) = &self.file_source {
            item.load_description(&**file_source, &self.run_loop);
        }
        Ok(())
    }

    pub(crate) fn remove_source(&mut self, id: &str) -> Option<Box<dyn Source>> {
        // Check if source is in use
        for layer in self.layers.iter() {
            if layer.get_source_id() == id {
                Log::warning(Event::General, format!("Source '{id}' is in use, cannot remove"));
                return None;
            }
        }

        let source = self.sources.remove(id);

        if let Some(source) = &source {
            source.set_observer(None);
            self.observer().on_update();
        }

        source
    }

    // Layers

    /// The layers, in drawing order.
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

    pub fn get_sky(&self) -> Option<&Arc<Sky>> {
        self.sky.as_ref()
    }

    // Projection

    pub(crate) fn set_projection(&mut self, projection: Projection) {
        self.projection = Arc::new(projection);
        self.on_projection_changed();
    }

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
    pub fn get_font_faces(&self) -> Option<Arc<mln_text::glyph::FontFaces>> {
        self.font_faces.clone()
    }

    pub fn get_glyph_url(&self) -> &str {
        &self.glyph_url
    }

    pub fn get_image_impls(&self) -> Arc<ImageImpls> {
        Arc::clone(&self.images)
    }

    /// The impls of the sources. A source whose impl changed since the
    /// collection last stored it gets its current impl (one new vector per
    /// change, as C++ makes one per update in its `SourceObserver`
    /// callbacks).
    pub fn get_source_impls(&self) -> ImmutableVector<dyn source::Impl> {
        let impls = self.sources.get_impls();
        if self.sources.are_current(&impls) {
            return impls;
        }
        let mut cache = self.source_impls_cache.borrow_mut();
        if let Some((base, refreshed)) = &*cache
            && Arc::ptr_eq(base, &impls)
            && self.sources.are_current(refreshed)
        {
            return Arc::clone(refreshed);
        }
        let refreshed: ImmutableVector<dyn source::Impl> = Arc::new(self.sources.current_impls());
        *cache = Some((impls, Arc::clone(&refreshed)));
        refreshed
    }

    /// Logs the sources and their state.
    pub fn dump_debug_logs(&self) {
        for source in self.sources.iter() {
            source.dump_debug_logs();
        }
    }

    /// Whether every source has loaded its description.
    pub fn sources_loaded(&self) -> bool {
        self.sources.iter().all(Source::loaded)
    }

    // The SourceObserver implementation is StyleSourceObserver.

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

/// Fails when `layer`'s source is in `sources` and doesn't support the
/// layer's type.
fn check_layer_source(sources: &CollectionWithPersistentOrder<dyn Source>, layer: &Layer) -> Result<(), Error> {
    // TODO: verify source
    let impl_ = layer.style();
    if let Some(source) = sources.get(&impl_.base().source)
        && !source.supports_layer_type(impl_.info())
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
