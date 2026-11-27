use std::sync::Arc;

use crate::conversion::Convertible;
use crate::error::Error;
use crate::expression::GlobalStateMap;
use crate::feature::Value;
use crate::filter::Filter;
use crate::font_face::FontFaces;
use crate::geo::LatLng;
use crate::layer::{Layer, LayerList, SourceKind};
use crate::light::Light;
use crate::projection::Projection;
use crate::property::transition::TransitionOptions;
use crate::registry::StyleRegistry;
use crate::sky::Sky;
use crate::source::{Source, SourceList};
use crate::sprite::Sprite;
use crate::style::edit::Edit;
use crate::style::parse::Parser;
use crate::terrain::Terrain;
use crate::{image, log_target};

pub mod diff;
pub(crate) mod edit;
pub(crate) mod json;
pub(crate) mod parse;

/// The camera that a style's root properties set: `center`, `centerAltitude`,
/// `zoom`, `bearing`, `pitch` and `roll`. A parsed style sets every member,
/// to the style spec's default where the document leaves it out; an empty
/// style sets none.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StyleCamera {
    pub center: Option<LatLng>,
    /// In meters above sea level.
    pub center_altitude: Option<f64>,
    pub zoom: Option<f64>,
    /// In degrees.
    pub bearing: Option<f64>,
    /// In degrees.
    pub pitch: Option<f64>,
    /// In degrees.
    pub roll: Option<f64>,
}

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

/// The images of a style, sorted by id.
pub(crate) type ImageList = Vec<Arc<image::Impl>>;

/// A version of a map's style: what a style document describes, after the
/// edits made since.
///
/// A style is a value. [`apply`](Self::apply) makes an [`Edit`]; nothing
/// else changes it. Its parts (layers, sources, images, light, sky,
/// terrain, projection, global state and the document's own members) sit behind
/// their own `Arc`s, so a clone is cheap and an edit copies only the part it
/// changes. Two versions whose part `Arc`s are the same pointer have the
/// same part, which [`is_same_version`](Self::is_same_version) and
/// [`StyleDiff::between`](crate::diff::StyleDiff::between) read.
#[derive(Clone)]
pub struct Style {
    registry: Arc<StyleRegistry>,
    /// The document, for the root properties that edits don't change.
    json: Arc<str>,
    name: Arc<str>,
    default_camera: StyleCamera,
    layers: Arc<LayerList>,
    /// The sources, in the order they were added.
    sources: Arc<SourceList>,
    images: Arc<ImageList>,
    light: Arc<Light>,
    sky: Option<Arc<Sky>>,
    terrain: Option<Arc<Terrain>>,
    projection: Arc<Projection>,
    transition: TransitionOptions,
    /// What the `global-state` expressions read.
    global_state: Arc<GlobalStateMap>,
    /// The defaults that the document's root `state` gives.
    global_state_defaults: Arc<GlobalStateMap>,
    sprites: Arc<[Sprite]>,
    glyph_url: Arc<str>,
    /// The document's `font-faces`, if it has them.
    font_faces: Option<Arc<FontFaces>>,
}

// The map and a renderer on another thread share versions.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Style>();
};

impl std::fmt::Debug for Style {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Style")
            .field("name", &self.name)
            .field("layers", &self.layers)
            .field("sources", &self.sources)
            .finish_non_exhaustive()
    }
}

impl Style {
    /// An empty style whose JSON, layers and filters parse against
    /// `registry`.
    #[must_use]
    pub fn new(registry: Arc<StyleRegistry>) -> Self {
        Style {
            registry,
            json: Arc::from(""),
            name: Arc::from(""),
            default_camera: StyleCamera::default(),
            layers: Arc::default(),
            sources: Arc::default(),
            images: Arc::default(),
            light: Arc::default(),
            sky: None,
            terrain: None,
            projection: Arc::default(),
            transition: TransitionOptions::default(),
            global_state: Arc::default(),
            global_state_defaults: Arc::default(),
            sprites: Arc::from([]),
            glyph_url: Arc::from(""),
            font_faces: None,
        }
    }

    /// The style that the style document `json` describes, parsed against
    /// `registry`. Fails with why `json` isn't a style.
    pub fn from_json(registry: &Arc<StyleRegistry>, json: &str) -> Result<Style, Error> {
        let mut parser = Parser::new(registry);
        parser.parse(json)?;

        let mut style = Style::new(Arc::clone(registry));
        style.json = Arc::from(json);
        style.transition = parser.transition;
        style.global_state_defaults = Arc::new(std::mem::take(&mut parser.global_state_defaults));
        style.global_state = Arc::clone(&style.global_state_defaults);

        let mut sources = SourceList::default();
        for source in std::mem::take(&mut parser.sources) {
            // A parsed style cannot contain duplicate source ids (they are
            // object keys).
            if sources.get(source.id()).is_some() {
                log::error!(target: log_target::PARSE_STYLE, "Source {} already exists", source.id());
                continue;
            }
            sources.set(source);
        }
        style.sources = Arc::new(sources);

        // The parser drops duplicate layer ids, so the layers go in at once
        // without `AddLayer`'s lookup. A layer incompatible with its source
        // is logged and skipped.
        let global_state = Arc::clone(&style.global_state);
        let mut layers = LayerList::default();
        layers.extend(std::mem::take(&mut parser.layers).into_iter().filter_map(|mut layer| {
            if let Err(error) = check_layer_source(&style.sources, &layer) {
                log::error!(target: log_target::PARSE_STYLE, "{error}");
                return None;
            }
            layer.reevaluate_visibility(&global_state);
            Some(layer)
        }));
        style.layers = Arc::new(layers);

        style.name = Arc::from(parser.name.as_str());
        style.default_camera = StyleCamera {
            center: Some(parser.lat_lng),
            center_altitude: Some(parser.center_altitude),
            zoom: Some(parser.zoom),
            bearing: Some(parser.bearing),
            pitch: Some(parser.pitch),
            roll: Some(parser.roll),
        };

        style.light = Arc::new(std::mem::take(&mut parser.light));
        style.sky = parser.sky.take().map(Arc::new);
        style.terrain = parser
            .terrain
            .take()
            .filter(|terrain| match check_terrain(&style.sources, terrain) {
                Ok(()) => true,
                Err(error) => {
                    log::error!(target: log_target::PARSE_STYLE, "{error}");
                    false
                }
            })
            .map(Arc::new);
        style.projection = Arc::new(std::mem::take(&mut parser.projection));

        style.sprites = Arc::from(std::mem::take(&mut parser.sprites));
        style.glyph_url = Arc::from(parser.glyph_url.as_str());
        style.font_faces = parser.font_faces.take();
        Ok(style)
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<StyleRegistry> {
        &self.registry
    }

    /// The style document, without the edits made since it parsed.
    #[must_use]
    pub fn get_json(&self) -> &str {
        &self.json
    }

    /// The document's sprites.
    #[must_use]
    pub fn get_sprites(&self) -> &[Sprite] {
        &self.sprites
    }

    #[must_use]
    pub fn get_name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn get_default_camera(&self) -> StyleCamera {
        self.default_camera
    }

    #[must_use]
    pub fn get_glyph_url(&self) -> &str {
        &self.glyph_url
    }

    #[must_use]
    pub fn get_font_faces(&self) -> Option<&Arc<FontFaces>> {
        self.font_faces.as_ref()
    }

    #[must_use]
    pub fn get_transition_options(&self) -> TransitionOptions {
        self.transition
    }

    #[must_use]
    pub fn get_light(&self) -> &Arc<Light> {
        &self.light
    }

    #[must_use]
    pub fn get_sky(&self) -> Option<&Arc<Sky>> {
        self.sky.as_ref()
    }

    #[must_use]
    pub fn get_terrain(&self) -> Option<&Arc<Terrain>> {
        self.terrain.as_ref()
    }

    #[must_use]
    pub fn get_projection(&self) -> &Arc<Projection> {
        &self.projection
    }

    /// What the style's `global-state` expressions read.
    #[must_use]
    pub fn get_global_state(&self) -> &Arc<GlobalStateMap> {
        &self.global_state
    }

    /// The defaults that the style's root `state` gives.
    #[must_use]
    pub fn get_global_state_defaults(&self) -> &Arc<GlobalStateMap> {
        &self.global_state_defaults
    }

    /// The images, sorted by id.
    #[must_use]
    pub fn get_images(&self) -> &Arc<Vec<Arc<image::Impl>>> {
        &self.images
    }

    #[must_use]
    pub fn get_image(&self, id: &str) -> Option<&Arc<image::Impl>> {
        self.images.iter().find(|image| image.id == id)
    }

    /// The style's sources, in the order they were added.
    #[must_use]
    pub fn get_sources(&self) -> &Arc<SourceList> {
        &self.sources
    }

    /// The source `id`.
    #[must_use]
    pub fn get_source(&self, id: &str) -> Option<&Source> {
        self.sources.get(id)
    }

    /// The layers in drawing order, bottom first.
    #[must_use]
    pub fn get_layer_list(&self) -> &Arc<LayerList> {
        &self.layers
    }

    /// The layers, bottom first.
    pub fn get_layers(&self) -> impl DoubleEndedIterator<Item = &Layer> + ExactSizeIterator {
        self.layers.iter()
    }

    /// The place of the layer `id` in drawing order, bottom first.
    #[must_use]
    pub fn layer_index(&self, layer_id: &str) -> Option<usize> {
        self.layers.index_of(layer_id)
    }

    #[must_use]
    pub fn get_layer(&self, layer_id: &str) -> Option<&Layer> {
        self.layers.get(layer_id)
    }

    /// Makes `edit`. Fails with [`ErrorKind::NotFound`](crate::error::ErrorKind)
    /// for a layer the style lacks, with `AlreadyExists` for a layer id it
    /// has, and with `InvalidArgument` for a value that doesn't fit, and
    /// leaves the style as it was when it fails. An edit that changes
    /// nothing leaves every part's `Arc` as it was.
    pub fn apply(&mut self, edit: Edit) -> Result<(), Error> {
        match edit {
            Edit::AddLayer { mut layer, before } => {
                check_layer_source(&self.sources, &layer)?;
                if self.layers.get(layer.get_id()).is_some() {
                    return Err(Error::already_exists(format!("Layer {} already exists", layer.get_id())));
                }
                // The global state is not available while the layer itself is parsed.
                layer.reevaluate_visibility(&self.global_state);
                let inserted = Arc::make_mut(&mut self.layers).insert_before(layer, before.as_deref());
                debug_assert!(inserted.is_ok());
            }
            Edit::RemoveLayer(id) => {
                if self.layers.get(&id).is_none() {
                    return Err(layer_not_found(&id));
                }
                Arc::make_mut(&mut self.layers).remove(&id);
            }
            Edit::MoveLayer { id, before } => {
                if self.layers.get(&id).is_none() {
                    return Err(layer_not_found(&id));
                }
                if before.as_deref() == Some(id.as_str()) {
                    return Ok(());
                }
                Arc::make_mut(&mut self.layers).move_before(&id, before.as_deref());
            }
            Edit::SetLayer(layer) => {
                check_layer_source(&self.sources, &layer)?;
                let id = layer.get_id().to_owned();
                self.update_layer(&id, |old| {
                    *old = layer;
                    Ok(())
                })?;
            }
            Edit::SetLayerProperty { layer, name, value } => {
                let sources = Arc::clone(&self.sources);
                self.update_layer(&layer, |target| {
                    set_layer_property(target, &name, &value)?;
                    check_layer_source(&sources, target)
                })?;
            }
            Edit::SetLayerFilter { layer, filter } => {
                self.update_layer(&layer, |target| {
                    target.set_filter(&filter);
                    Ok(())
                })?;
            }
            Edit::SetLayerZoomRange { layer, min, max } => {
                self.update_layer(&layer, |target| {
                    target.set_min_zoom(min);
                    target.set_max_zoom(max);
                    Ok(())
                })?;
            }
            Edit::SetLayerVisibility { layer, visibility } => {
                self.update_layer(&layer, |target| {
                    target.set_visibility(visibility);
                    Ok(())
                })?;
            }
            Edit::SetLight(light) => self.light = Arc::new(light),
            Edit::SetLightProperty { name, value } => {
                let functions = Arc::clone(self.registry.functions());
                let mut light = Arc::clone(&self.light);
                if let Some(error) =
                    Light::set_shared_property(&mut light, &name, &Convertible::from(&value), &functions)
                {
                    let context = format!("style light property \"{name}\"");
                    return Err(Error::invalid_argument(format!("{context}: {}", error.message)));
                }
                self.light = light;
            }
            Edit::SetSky(sky) => self.sky = sky.map(Arc::new),
            Edit::SetTerrain(terrain) => {
                if let Some(terrain) = &terrain {
                    check_terrain(&self.sources, terrain)?;
                }
                if self.terrain.as_deref() != terrain.as_ref() {
                    self.terrain = terrain.map(Arc::new);
                }
            }
            Edit::SetProjection(projection) => self.projection = Arc::new(projection),
            Edit::SetTransition(options) => self.transition = options,
            Edit::SetGlobalState { name, value } => self.set_global_state(&name, &value),
            Edit::SetGlobalStateDefaults(defaults) => {
                self.global_state_defaults = Arc::new(defaults);
                self.global_state = Arc::clone(&self.global_state_defaults);
                self.reevaluate_layer_visibilities();
            }
            Edit::AddImage(image) => {
                let images = Arc::make_mut(&mut self.images);
                let index = images.partition_point(|other| other.id < image.id);
                if images.get(index).is_some_and(|other| other.id == image.id) {
                    images[index] = image;
                } else {
                    images.insert(index, image);
                }
            }
            Edit::RemoveImage(id) => {
                let Some(index) = self.images.iter().position(|image| image.id == id) else {
                    log::warn!(target: log_target::GENERAL, "Image '{id}' is not present in style, cannot remove");
                    return Ok(());
                };
                Arc::make_mut(&mut self.images).remove(index);
            }
            Edit::AddImages(images) => self.add_images(images),
            Edit::AddSource(source) => {
                if self.sources.get(source.id()).is_some() {
                    return Err(Error::already_exists(format!("source \"{}\" already exists", source.id())));
                }
                for layer in self.layers.iter().filter(|layer| layer.get_source_id() == source.id()) {
                    check_source_feeds(&source, layer)?;
                }
                Arc::make_mut(&mut self.sources).set(source);
            }
            Edit::RemoveSource(id) => {
                if self.sources.get(&id).is_none() {
                    return Err(source_not_found(&id));
                }
                Arc::make_mut(&mut self.sources).remove(&id);
            }
            Edit::SetSourceDescription { id, instance, load, description } => {
                let Some(source) = self.sources.get(&id) else { return Ok(()) };
                if source.instance() != instance || source.data().load() != load {
                    return Ok(());
                }
                let source = source.with_description(description);
                Arc::make_mut(&mut self.sources).set(source);
            }
            Edit::SetSourceCommon { id, common } => {
                let source = self.sources.get(&id).ok_or_else(|| source_not_found(&id))?;
                if *source.common() != common {
                    let source = source.with_common(common);
                    Arc::make_mut(&mut self.sources).set(source);
                }
            }
            Edit::UpdateSource { id, update } => {
                let source = self.sources.get(&id).ok_or_else(|| source_not_found(&id))?;
                let instance = source.instance();
                if let Some(source) = update(source)? {
                    debug_assert!(source.id() == id && source.instance() == instance, "an update keeps its source");
                    Arc::make_mut(&mut self.sources).set(source);
                }
            }
        }
        Ok(())
    }

    /// Whether `other` is the same version as `self`: every part is the same
    /// `Arc`, and the transitions are equal. O(1): it walks nothing.
    #[must_use]
    pub fn is_same_version(&self, other: &Style) -> bool {
        fn same_option<T: ?Sized>(a: Option<&Arc<T>>, b: Option<&Arc<T>>) -> bool {
            match (a, b) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
        }
        Arc::ptr_eq(&self.layers, &other.layers)
            && Arc::ptr_eq(&self.sources, &other.sources)
            && Arc::ptr_eq(&self.images, &other.images)
            && Arc::ptr_eq(&self.light, &other.light)
            && same_option(self.sky.as_ref(), other.sky.as_ref())
            && same_option(self.terrain.as_ref(), other.terrain.as_ref())
            && Arc::ptr_eq(&self.projection, &other.projection)
            && self.transition == other.transition
            && Arc::ptr_eq(&self.global_state, &other.global_state)
            && Arc::ptr_eq(&self.registry, &other.registry)
            && Arc::ptr_eq(&self.json, &other.json)
            && Arc::ptr_eq(&self.name, &other.name)
            && self.default_camera == other.default_camera
            && Arc::ptr_eq(&self.sprites, &other.sprites)
            && Arc::ptr_eq(&self.glyph_url, &other.glyph_url)
            && same_option(self.font_faces.as_ref(), other.font_faces.as_ref())
            && Arc::ptr_eq(&self.global_state_defaults, &other.global_state_defaults)
    }

    /// Calls `f` with a copy of the layer `id`, and keeps the copy if `f`
    /// succeeded and changed the layer's style data, with its visibility
    /// re-evaluated.
    fn update_layer(&mut self, id: &str, f: impl FnOnce(&mut Layer) -> Result<(), Error>) -> Result<(), Error> {
        let Some((index, layer)) = self.layers.get_full(id) else {
            return Err(layer_not_found(id));
        };
        let mut copy = layer.clone();
        f(&mut copy)?;
        if !Arc::ptr_eq(layer.style(), copy.style()) {
            copy.reevaluate_visibility(&self.global_state);
            Arc::make_mut(&mut self.layers).replace(index, copy);
        }
        Ok(())
    }

    /// Sets a global state property. A null value resets the property to the
    /// default that the style's root `state` gives (or null).
    fn set_global_state(&mut self, property: &str, value: &Value) {
        let mut new_value = value;
        if matches!(value, Value::Null)
            && let Some(default) = self.global_state_defaults.get(property)
        {
            new_value = default;
        }
        if self.global_state.get(property).is_some_and(|current| state_values_equal(current, new_value)) {
            return;
        }
        Arc::make_mut(&mut self.global_state).insert(property.to_owned(), new_value.clone());
        self.reevaluate_layer_visibilities();
    }

    /// Re-evaluates the visibility expressions of all layers against the
    /// current global state.
    fn reevaluate_layer_visibilities(&mut self) {
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
        let layers = Arc::make_mut(&mut self.layers);
        for (index, layer) in changed {
            layers.replace(index, layer);
        }
    }

    /// Adds a sprite's images, each replacing the image with its id.
    fn add_images(&mut self, images: Vec<Arc<image::Impl>>) {
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
}

fn source_not_found(id: &str) -> Error {
    Error::not_found(format!("source \"{id}\" does not exist"))
}

/// Sets the property `name` of `layer` to the style JSON `value`: a layer
/// type's property or transition, or `filter`, `minzoom`, `maxzoom`,
/// `source` or `source-layer`, where null resets the ones with a default.
fn set_layer_property(layer: &mut Layer, name: &str, value: &serde_json::Value) -> Result<(), Error> {
    let type_info = layer.info();
    let id = layer.get_id().to_owned();
    if (name == "source" || name == "source-layer") && type_info.source == SourceKind::None {
        // A layer that takes no source would ignore the value.
        return Err(Error::invalid_argument(format!("{} layer \"{id}\" does not take a {name}", type_info.name)));
    }
    match name {
        "filter" if value.is_null() => layer.set_filter(&Filter::default()),
        "minzoom" if value.is_null() => layer.set_min_zoom(f32::NEG_INFINITY),
        "maxzoom" if value.is_null() => layer.set_max_zoom(f32::INFINITY),
        _ => {
            if let Some(error) = layer.set_property(name, &Convertible::from(value), None) {
                let context = format!("property \"{name}\" of layer \"{id}\"");
                return Err(Error::invalid_argument(format!("{context}: {}", error.message)));
            }
        }
    }
    Ok(())
}

fn layer_not_found(id: &str) -> Error {
    Error::not_found(format!("layer \"{id}\" does not exist"))
}

/// Fails when `layer`'s source is in `sources` and doesn't feed layers of
/// the layer's type.
fn check_layer_source(sources: &SourceList, layer: &Layer) -> Result<(), Error> {
    match sources.get(layer.get_source_id()) {
        Some(source) => check_source_feeds(source, layer),
        None => Ok(()),
    }
}

/// Fails when `layer` takes a source and `source` doesn't feed layers of its
/// type.
fn check_source_feeds(source: &Source, layer: &Layer) -> Result<(), Error> {
    let kind = layer.info().source;
    if kind != SourceKind::None && source.info().layer_kind != kind {
        let message = format!("Layer '{}' is not compatible with source '{}'", layer.get_id(), source.id());
        return Err(Error::invalid_argument(message));
    }
    Ok(())
}

/// Fails when `terrain`'s exaggeration isn't a finite number of at least 0, or
/// its source is in `sources` and isn't a `raster-dem` source. A source the
/// style lacks passes: the terrain waits for a source with that id.
fn check_terrain(sources: &SourceList, terrain: &Terrain) -> Result<(), Error> {
    if !(terrain.exaggeration >= 0.0 && terrain.exaggeration.is_finite()) {
        return Err(Error::invalid_argument("Terrain exaggeration must be a finite number of at least 0"));
    }
    if let Some(source) = sources.get(&terrain.source)
        && source.info().layer_kind != SourceKind::RasterDem
    {
        let message = format!("Terrain source '{}' is not a raster-dem source", terrain.source);
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
