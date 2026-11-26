//! The style's JSON-text entry points: they take and return style-spec JSON
//! as text, so a caller on another thread hands over text and the style's
//! own thread parses it.

use std::sync::Arc;
use std::time::Duration;

use feature::Value;

use crate::constants::DEFAULT_TRANSITION_DURATION;
use crate::conversion::borrowed::{Json, borrowed_json_to_value, member, members};
use crate::conversion::{Converter, Convertible, value_to_json};
use crate::error::Error;
use crate::expression::GlobalStateMap;
use crate::layer::{Layer, SourceKind, convert_layer};
use crate::light::{Light, convert_light};
use crate::projection::{Projection, convert_projection};
use crate::property::table::{Kind, Properties, StyleProperty, defined_properties};
use crate::property::transition::TransitionOptions;
use crate::sky::convert_sky;
use crate::style::Style;
use crate::style::edit::Edit;
use crate::terrain::convert_terrain;

impl Style {
    /// Adds the [layer](https://maplibre.org/maplibre-style-spec/layers/)
    /// that `json` describes, below the layer named `before_layer_id`, or
    /// above every layer when it is `None`.
    ///
    /// Fails with [`ErrorKind::NotFound`](crate::error::ErrorKind) if the style has no layer named
    /// `before_layer_id` or no source with the ID that the layer uses, with
    /// `AlreadyExists` if the style has a layer with the new layer's ID, and
    /// with `InvalidArgument` if `json` isn't a valid layer or its source
    /// can't feed a layer of its type.
    pub fn add_layer_json(&mut self, json: &str, before_layer_id: Option<&str>) -> Result<(), Error> {
        if let Some(before) = before_layer_id
            && self.get_layer(before).is_none()
        {
            return Err(Error::not_found(format!("before_layer_id {} does not exist", quote(before))));
        }

        let value = parse(json, "style layer")?;
        let mut error = crate::conversion::Error::default();
        let Some(layer) = convert_layer(self.registry(), &Convertible::from(&value), &mut error) else {
            return Err(conversion_error("style layer", &error));
        };

        self.add_layer(layer, before_layer_id)
    }

    /// Adds `layer` below the layer named `before_layer_id`, or above every
    /// layer when it is `None`.
    ///
    /// Fails as [`Style::add_layer_json`] does for a valid layer. The style
    /// drops `layer`, and what it keeps alive, when it fails.
    pub fn add_layer(&mut self, layer: Layer, before_layer_id: Option<&str>) -> Result<(), Error> {
        if let Some(before) = before_layer_id
            && self.get_layer(before).is_none()
        {
            return Err(Error::not_found(format!("before_layer_id {} does not exist", quote(before))));
        }
        let id = layer.get_id();
        if self.get_layer(id).is_some() {
            return Err(Error::already_exists(format!("layer {} already exists", quote(id))));
        }
        let type_info = layer.info();
        if type_info.source != SourceKind::None {
            let source_id = layer.get_source_id();
            let Some(source) = self.get_source(source_id) else {
                return Err(Error::not_found(format!(
                    "layer {} uses source {}, which does not exist",
                    quote(id),
                    quote(source_id)
                )));
            };
            if source.info().layer_kind != type_info.source {
                return Err(Error::invalid_argument(format!(
                    "source {} does not support {} layer {}",
                    quote(source_id),
                    type_info.name,
                    quote(id)
                )));
            }
        }
        self.apply(Edit::AddLayer { layer, before: before_layer_id.map(str::to_owned) })
    }

    /// Moves the layer named `id` below the layer named `before_layer_id`,
    /// or above every layer when it is `None`. A move below itself changes
    /// nothing.
    ///
    /// Fails with [`ErrorKind::NotFound`](crate::error::ErrorKind) if the style has no layer
    /// named `id` or `before_layer_id`.
    pub fn move_layer(&mut self, id: &str, before_layer_id: Option<&str>) -> Result<(), Error> {
        if self.get_layer(id).is_none() {
            return Err(layer_not_found(id));
        }
        if let Some(before) = before_layer_id {
            if before == id {
                return Ok(());
            }
            if self.get_layer(before).is_none() {
                return Err(Error::not_found(format!("before_layer_id {} does not exist", quote(before))));
            }
        }
        self.apply(Edit::MoveLayer { id: id.to_owned(), before: before_layer_id.map(str::to_owned) })
    }

    /// The layer named `id` as a
    /// [layer](https://maplibre.org/maplibre-style-spec/layers/) object, or
    /// `None` if the style has no such layer. Adding the JSON back gives the
    /// same layer.
    #[must_use]
    pub fn layer_json(&self, id: &str) -> Option<String> {
        self.get_layer(id).map(|layer| json_text(&layer.serialize()))
    }

    /// Sets the property `name` of the layer named `layer_id` to the JSON
    /// value `json`.
    ///
    /// `name` is one of the layer's layout or paint
    /// [properties](https://maplibre.org/maplibre-style-spec/layers/), or
    /// `filter`, `minzoom`, `maxzoom`, `source` or `source-layer`. A `null`
    /// resets any of these but `source` and `source-layer` to its default.
    ///
    /// Fails with [`ErrorKind::NotFound`](crate::error::ErrorKind) if the style has no layer named
    /// `layer_id`, and with `InvalidArgument` if `json` isn't valid JSON,
    /// the layer has no property `name` or the value doesn't fit it.
    pub fn set_layer_property_json(&mut self, layer_id: &str, name: &str, json: &str) -> Result<(), Error> {
        let context = format!("property {} of layer {}", quote(name), quote(layer_id));
        let value = parse_value(json, &context)?;
        self.apply(Edit::SetLayerProperty { layer: layer_id.to_owned(), name: name.to_owned(), value })
    }

    /// The JSON value of the property `name` of the layer named `layer_id`,
    /// or `None` if the layer doesn't set it. `name` is one of the names
    /// [`set_layer_property_json`](Self::set_layer_property_json) takes. The
    /// value can differ in form from the value that was set: a color set as
    /// `"#ff0000"` reads back as `["rgba",255.0,0.0,0.0,1.0]`.
    ///
    /// Fails with [`ErrorKind::NotFound`](crate::error::ErrorKind) if the style
    /// has no layer named `layer_id`.
    pub fn layer_property_json(&self, layer_id: &str, name: &str) -> Result<Option<String>, Error> {
        let Some(layer) = self.get_layer(layer_id) else {
            return Err(layer_not_found(layer_id));
        };
        let value = match name {
            "filter" => {
                let filter = layer.get_filter();
                filter.is_set().then(|| filter.serialize())
            }
            "source" | "source-layer" => {
                let source = if name == "source" { layer.get_source_id() } else { layer.get_source_layer() };
                (!source.is_empty()).then(|| Value::String(source.to_owned()))
            }
            "minzoom" => {
                let zoom = layer.get_min_zoom();
                (zoom != f32::NEG_INFINITY).then_some(Value::Double(zoom.into()))
            }
            "maxzoom" => {
                let zoom = layer.get_max_zoom();
                (zoom != f32::INFINITY).then_some(Value::Double(zoom.into()))
            }
            _ => {
                let property = layer.get_property(name);
                (property.get_kind() != Kind::Undefined).then(|| property.get_value().clone())
            }
        };
        Ok(value.as_ref().map(json_text))
    }

    /// Sets the [root property](https://maplibre.org/maplibre-style-spec/root/)
    /// `name` to the JSON value `json`, without reloading the style. A `null`
    /// resets it to its default, as if the style didn't set it.
    ///
    /// `light`, `sky`, `terrain`, `projection`, `state` and `transition` change. A
    /// `state` sets every global state property to its new default. A
    /// `transition` keeps whether placements fade, which the style spec's
    /// transition doesn't hold.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](crate::error::ErrorKind) if
    /// `name` isn't a root property, or is `sources` or `layers`, or if
    /// `json` isn't a valid value for it; and with `Unsupported` for the root properties that only
    /// take effect as a style loads (the default camera, the sprite, the
    /// glyphs and font faces, and the metadata).
    pub fn set_root_property_json(&mut self, name: &str, json: &str) -> Result<(), Error> {
        require_root_property(name)?;
        let context = format!("style root property {}", quote(name));
        let value = parse(json, &context)?;
        let convertible = Convertible::from(&value);
        let mut error = crate::conversion::Error::default();
        let functions = Arc::clone(self.registry().functions());
        match name {
            "light" => {
                let light = if value.is_null() {
                    Light::default()
                } else {
                    convert_light(&convertible, &functions, &mut error)
                        .ok_or_else(|| conversion_error(&context, &error))?
                };
                self.apply(Edit::SetLight(light))?;
            }
            "sky" => {
                let sky = if value.is_null() {
                    None
                } else {
                    Some(
                        convert_sky(&convertible, &functions, &mut error)
                            .ok_or_else(|| conversion_error(&context, &error))?,
                    )
                };
                self.apply(Edit::SetSky(sky))?;
            }
            "terrain" => {
                let terrain = if value.is_null() {
                    None
                } else {
                    Some(convert_terrain(&convertible, &mut error).ok_or_else(|| conversion_error(&context, &error))?)
                };
                self.apply(Edit::SetTerrain(terrain))?;
            }
            "projection" => {
                let projection = if value.is_null() {
                    Projection::default()
                } else {
                    convert_projection(&convertible, &functions, &mut error)
                        .ok_or_else(|| conversion_error(&context, &error))?
                };
                self.apply(Edit::SetProjection(projection))?;
            }
            "state" => {
                let defaults = if value.is_null() { GlobalStateMap::new() } else { state_defaults(&value, &context)? };
                self.apply(Edit::SetGlobalStateDefaults(defaults))?;
            }
            "transition" => {
                let placement = self.get_transition_options().enable_placement_transitions;
                let mut transition = if value.is_null() {
                    TransitionOptions::with_duration(DEFAULT_TRANSITION_DURATION)
                } else {
                    TransitionOptions::convert(&convertible, &mut error)
                        .ok_or_else(|| conversion_error(&context, &error))?
                };
                transition.enable_placement_transitions = placement;
                self.apply(Edit::SetTransition(transition))?;
            }
            _ => return Err(Error::unsupported(format!("{context} can't change while the style is loaded"))),
        }
        Ok(())
    }

    /// The JSON value of the [root property](https://maplibre.org/maplibre-style-spec/root/)
    /// `name`, or `None` if neither the style nor the map sets it: the map
    /// gives a style without a `transition` a default duration. The value can
    /// differ in form from the value that was set, as with
    /// [`layer_property_json`](Self::layer_property_json). The root
    /// properties that [`set_root_property_json`](Self::set_root_property_json)
    /// can't change read as the style loaded them. `terrain` reads as the
    /// style holds it: with its default exaggeration filled in, and absent if
    /// the style dropped it as it loaded.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](crate::error::ErrorKind) if
    /// `name` isn't a root property, or is `sources` or `layers`.
    pub fn root_property_json(&self, name: &str) -> Result<Option<String>, Error> {
        require_root_property(name)?;
        let value = match name {
            "light" => defined_properties_json(&**self.get_light()),
            "sky" => self.get_sky().and_then(|sky| defined_properties_json(&**sky)),
            "terrain" => self.get_terrain().map(|terrain| {
                serde_json::json!({ "source": terrain.source, "exaggeration": shortest_f64(terrain.exaggeration) })
            }),
            "projection" => {
                let projection = self.get_projection();
                properties_json(&["type"], |name| projection.get_property(name))
            }
            "state" => {
                let defaults = self.get_global_state_defaults();
                (!defaults.is_empty()).then(|| {
                    let definitions = defaults.iter().map(|(name, default)| {
                        (name.clone(), serde_json::json!({ "default": value_to_json(default) }))
                    });
                    serde_json::Value::Object(definitions.collect())
                })
            }
            "transition" => {
                let transition = self.get_transition_options();
                let members = [("duration", transition.duration), ("delay", transition.delay)]
                    .into_iter()
                    .filter_map(|(name, duration)| Some((name.to_owned(), milliseconds_json(duration?))))
                    .collect::<serde_json::Map<_, _>>();
                (!members.is_empty()).then_some(serde_json::Value::Object(members))
            }
            _ => serde_json::from_str::<serde_json::Value>(self.get_json())
                .ok()
                .and_then(|mut document| document.get_mut(name).map(serde_json::Value::take)),
        };
        Ok(value.map(|value| value.to_string()))
    }

    /// Sets the property `name` of the style's light to the JSON value
    /// `json`. A `null` resets it to its default.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](crate::error::ErrorKind) if `json` isn't valid
    /// JSON, the light has no property `name` or the value doesn't fit it.
    pub fn set_light_property_json(&mut self, name: &str, json: &str) -> Result<(), Error> {
        let context = format!("style light property {}", quote(name));
        let value = parse_value(json, &context)?;
        self.apply(Edit::SetLightProperty { name: name.to_owned(), value })
    }

    /// The JSON value of the property `name` of the style's light, or `None`
    /// if the light doesn't set it. The value can differ in form from the
    /// value that was set, as with
    /// [`layer_property_json`](Self::layer_property_json).
    #[must_use]
    pub fn light_property_json(&self, name: &str) -> Option<String> {
        let property = self.get_light().get_property(name);
        (property.get_kind() != Kind::Undefined).then(|| json_text(property.get_value()))
    }

    /// Sets the value `name` that the style's `global-state` expressions
    /// read to the JSON value `json`. A `null` resets it to the default that
    /// the style's `state` gives it.
    ///
    /// Fails with [`ErrorKind::InvalidArgument`](crate::error::ErrorKind) if `json` isn't valid
    /// JSON.
    pub fn set_global_state_property_json(&mut self, name: &str, json: &str) -> Result<(), Error> {
        let value = parse(json, &format!("global state property {}", quote(name)))?;
        self.apply(Edit::SetGlobalState { name: name.to_owned(), value: borrowed_json_to_value(&value) })
    }

    /// The values that the style's `global-state` expressions read, as a
    /// JSON object.
    #[must_use]
    pub fn global_state_json(&self) -> String {
        json_text(&Value::Object((**self.get_global_state()).clone()))
    }
}

/// The style spec's root properties, other than `sources` and `layers`,
/// which the source and layer calls change.
const ROOT_PROPERTIES: &[&str] = &[
    "version",
    "name",
    "metadata",
    "center",
    "centerAltitude",
    "zoom",
    "bearing",
    "pitch",
    "roll",
    "state",
    "light",
    "sky",
    "projection",
    "terrain",
    "sprite",
    "glyphs",
    "font-faces",
    "transition",
];

fn require_root_property(name: &str) -> Result<(), Error> {
    if ROOT_PROPERTIES.contains(&name) {
        Ok(())
    } else {
        Err(Error::invalid_argument(format!("{} is not a style root property that this call takes", quote(name))))
    }
}

/// An object of the properties of `owner` that are set, or `None` when none
/// is.
fn defined_properties_json<S: Properties>(owner: &S) -> Option<serde_json::Value> {
    let members = defined_properties(owner)
        .into_iter()
        .map(|(name, _, property)| (name, value_to_json(property.get_value())))
        .collect::<serde_json::Map<_, _>>();
    (!members.is_empty()).then_some(serde_json::Value::Object(members))
}

/// An object of the properties that are set, or `None` when none is.
fn properties_json<'a>(names: &[&'a str], property: impl Fn(&'a str) -> StyleProperty) -> Option<serde_json::Value> {
    let members = names
        .iter()
        .filter_map(|&name| {
            let property = property(name);
            (property.get_kind() != Kind::Undefined).then(|| (name.to_owned(), value_to_json(property.get_value())))
        })
        .collect::<serde_json::Map<_, _>>();
    (!members.is_empty()).then_some(serde_json::Value::Object(members))
}

/// Whole milliseconds as an integer, and finer durations with their
/// fraction.
fn milliseconds_json(duration: Duration) -> serde_json::Value {
    if duration.subsec_nanos().is_multiple_of(1_000_000) {
        serde_json::Value::from(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
    } else {
        serde_json::Value::from(duration.as_nanos() as f64 / 1e6)
    }
}

/// `value` as the `f64` with the same shortest decimal digits, so that an
/// `f32` of 0.3 reads back as 0.3.
fn shortest_f64(value: f32) -> f64 {
    value.to_string().parse().unwrap_or_else(|_| f64::from(value))
}

/// The defaults of a root `state` object: each member is an object, whose
/// `default` is the property's default.
fn state_defaults(value: &Json<'_>, context: &str) -> Result<GlobalStateMap, Error> {
    let Some(object) = value.as_object() else {
        return Err(Error::invalid_argument(format!("{context}: state must be an object")));
    };
    let mut defaults = GlobalStateMap::new();
    for (name, definition) in members(object) {
        let Some(definition) = definition.as_object() else {
            return Err(Error::invalid_argument(format!(
                "{context}: state property {} must be an object",
                quote(name)
            )));
        };
        if let Some(default) = member(definition, "default") {
            defaults.insert(name.to_owned(), borrowed_json_to_value(default));
        }
    }
    Ok(defaults)
}

fn quote(id: &str) -> String {
    format!("\"{id}\"")
}

/// `json` as an owned value, for an [`Edit`].
fn parse_value(json: &str, context: &str) -> Result<serde_json::Value, Error> {
    serde_json::from_str(json).map_err(|error| Error::invalid_argument(format!("{context} is not valid JSON: {error}")))
}

fn parse<'a>(json: &'a str, context: &str) -> Result<Json<'a>, Error> {
    serde_json::from_str(json).map_err(|error| Error::invalid_argument(format!("{context} is not valid JSON: {error}")))
}

fn conversion_error(context: &str, error: &crate::conversion::Error) -> Error {
    Error::invalid_argument(format!("{context}: {}", error.message))
}

fn layer_not_found(id: &str) -> Error {
    Error::not_found(format!("layer {} does not exist", quote(id)))
}

fn json_text(value: &Value) -> String {
    value_to_json(value).to_string()
}
