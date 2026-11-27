// The style's text is read into a `serde_json_borrow` tree, whose strings
// borrow from the text and whose objects keep their members in document
// order (conversion/borrowed.rs). The message of a JSON
// syntax error is serde_json's.

use std::sync::Arc;

use crate::collections::VecSet;
use crate::constants::DEFAULT_TRANSITION_DURATION;
use crate::conversion::borrowed::{Json, JsonObject, borrowed_json_to_value, member, members};
use crate::conversion::{Convertible, Error, convert};
use crate::expression::GlobalStateMap;
use crate::font_face::{FontFace, FontFaceRange, FontFaces};
use crate::geo::LatLng;
use crate::hash::{HashMap, HashSet};
use crate::layer::{FontStack, InvalidProperty, Layer, convert_layer_with, set_paint_properties};
use crate::light::{Light, convert_light};
use crate::log_target;
use crate::projection::{Projection, convert_projection};
use crate::property::transition::TransitionOptions;
use crate::registry::StyleRegistry;
use crate::sky::{Sky, convert_sky};
use crate::source::{Source, convert_source};
use crate::sprite::Sprite;
use crate::terrain::{Terrain, convert_terrain};

/// What `Parser::parse` returns: a style that isn't one fails with
/// [`ErrorKind::InvalidArgument`](crate::error::ErrorKind).
pub(crate) type StyleParseResult = Result<(), crate::error::Error>;

/// Reads a style's JSON, against the expression functions and layer types of
/// a [`StyleRegistry`].
pub struct Parser {
    registry: Arc<StyleRegistry>,
    pub sprites: Vec<Sprite>,
    pub glyph_url: String,
    pub font_faces: Option<Arc<FontFaces>>,
    pub sources: Vec<Source>,
    pub(crate) layers: Vec<Layer>,

    pub(crate) transition: TransitionOptions,
    pub(crate) light: Light,
    pub(crate) sky: Option<Sky>,
    pub(crate) terrain: Option<Terrain>,
    pub(crate) projection: Projection,

    /// Default values of the root "state" property, keyed by state property
    /// name, used by the "global-state" expression.
    pub(crate) global_state_defaults: GlobalStateMap,

    pub(crate) name: String,
    pub(crate) lat_lng: LatLng,
    pub(crate) zoom: f64,
    pub(crate) bearing: f64,
    pub(crate) pitch: f64,
    pub(crate) roll: f64,
    pub(crate) center_altitude: f64,
}

impl Parser {
    /// A parser of styles whose layers and expressions are `registry`'s.
    #[must_use]
    pub fn new(registry: &Arc<StyleRegistry>) -> Self {
        Parser {
            registry: Arc::clone(registry),
            sprites: Vec::new(),
            glyph_url: String::new(),
            font_faces: None,
            sources: Vec::new(),
            layers: Vec::new(),
            transition: TransitionOptions::with_duration(DEFAULT_TRANSITION_DURATION),
            light: Light::new(),
            sky: None,
            terrain: None,
            projection: Projection::new(),
            global_state_defaults: GlobalStateMap::new(),
            name: String::new(),
            lat_lng: LatLng::default(),
            zoom: 0.0,
            bearing: 0.0,
            pitch: 0.0,
            roll: 0.0,
            center_altitude: 0.0,
        }
    }
}

fn is_number(value: &Json<'_>) -> bool {
    value.is_number()
}

fn get_double(value: &Json<'_>) -> f64 {
    value.as_f64().unwrap_or(0.0)
}

impl Parser {
    pub fn parse(&mut self, json: &str) -> StyleParseResult {
        let document: Json<'_> = match serde_json::from_str(json) {
            Ok(document) => document,
            Err(error) => return Err(crate::error::Error::invalid_argument(format_json_parse_error(&error))),
        };

        let Some(object) = document.as_object() else {
            return Err(crate::error::Error::invalid_argument("style must be an object"));
        };

        if let Some(version_value) = member(object, "version") {
            // rapidjson GetInt() on a number.
            let version = if is_number(version_value) {
                version_value.as_i64().unwrap_or_else(|| get_double(version_value) as i64)
            } else {
                0
            };
            if version != 8 {
                log::warn!(
                    target: log_target::PARSE_STYLE,
                    "current renderer implementation only supports style spec version 8; using an outdated style will cause rendering errors"
                );
            }
        }

        if let Some(value) = member(object, "name")
            && let Some(name) = value.as_str()
        {
            self.name = name.to_owned();
        }

        if let Some(value) = member(object, "center") {
            let mut error = Error::default();
            match convert::<LatLng>(&Convertible::from(value), &mut error) {
                Some(lat_lng) => self.lat_lng = lat_lng,
                None => {
                    log::warn!(target: log_target::PARSE_STYLE, "center coordinate must be a longitude, latitude pair");
                }
            }
        }

        if let Some(value) = member(object, "centerAltitude")
            && is_number(value)
        {
            self.center_altitude = get_double(value);
        }

        if let Some(value) = member(object, "zoom")
            && is_number(value)
        {
            self.zoom = get_double(value);
        }

        if let Some(value) = member(object, "bearing")
            && is_number(value)
        {
            self.bearing = get_double(value);
        }

        if let Some(value) = member(object, "pitch")
            && is_number(value)
        {
            self.pitch = get_double(value);
        }

        if let Some(value) = member(object, "roll")
            && is_number(value)
        {
            self.roll = get_double(value);
        }

        if let Some(value) = member(object, "transition") {
            self.parse_transition(value);
        }

        if let Some(value) = member(object, "light") {
            self.parse_light(value);
        }

        if let Some(value) = member(object, "terrain") {
            self.parse_terrain(value);
        }
        if let Some(value) = member(object, "sky") {
            self.parse_sky(value);
        }

        if let Some(value) = member(object, "projection") {
            self.parse_projection(value);
        }

        if let Some(value) = member(object, "state") {
            self.parse_state(value);
        }

        if let Some(value) = member(object, "sources") {
            self.parse_sources(value);
        }

        if let Some(value) = member(object, "layers") {
            self.parse_layers(value);
        }

        if let Some(value) = member(object, "sprite") {
            self.parse_sprites(value);
        }

        if let Some(glyphs) = member(object, "glyphs")
            && let Some(glyphs) = glyphs.as_str()
        {
            self.glyph_url = glyphs.to_owned();
        }

        if let Some(faces) = member(object, "font-faces")
            && let Some(faces) = faces.as_object()
        {
            self.font_faces = Some(Arc::new(parse_font_faces(faces)));
        }

        // Call for side effect of logging warnings for invalid values.
        let _ = self.font_stacks();

        Ok(())
    }

    fn parse_transition(&mut self, value: &Json<'_>) {
        let mut error = Error::default();
        let Some(converted) = convert::<TransitionOptions>(&Convertible::from(value), &mut error) else {
            log::warn!(target: log_target::PARSE_STYLE, "{}", error.message);
            return;
        };

        self.transition = converted;
    }

    fn parse_light(&mut self, value: &Json<'_>) {
        let mut error = Error::default();
        let Some(converted) = convert_light(&Convertible::from(value), self.registry.functions(), &mut error) else {
            log::warn!(target: log_target::PARSE_STYLE, "{}", error.message);
            return;
        };

        self.light = converted;
    }

    fn parse_sky(&mut self, value: &Json<'_>) {
        let mut error = Error::default();
        let Some(converted) = convert_sky(&Convertible::from(value), self.registry.functions(), &mut error) else {
            log::warn!(target: log_target::PARSE_STYLE, "{}", error.message);
            return;
        };

        self.sky = Some(converted);
    }

    fn parse_terrain(&mut self, value: &Json<'_>) {
        let mut error = Error::default();
        let Some(converted) = convert_terrain(&Convertible::from(value), &mut error) else {
            log::warn!(target: log_target::PARSE_STYLE, "{}", error.message);
            return;
        };

        self.terrain = Some(converted);
    }

    fn parse_projection(&mut self, value: &Json<'_>) {
        let mut error = Error::default();
        let Some(converted) = convert_projection(&Convertible::from(value), self.registry.functions(), &mut error)
        else {
            log::warn!(target: log_target::PARSE_STYLE, "{}", error.message);
            return;
        };

        self.projection = converted;
    }

    fn parse_state(&mut self, value: &Json<'_>) {
        let Some(object) = value.as_object() else {
            log::warn!(target: log_target::PARSE_STYLE, "state must be an object");
            return;
        };

        for (property_name, definition) in members(object) {
            let Some(member_object) = definition.as_object() else {
                log::warn!(target: log_target::PARSE_STYLE, "state property \"{property_name}\" must be an object");
                continue;
            };
            let Some(default) = member(member_object, "default") else {
                continue;
            };
            // emplace: the first definition wins.
            self.global_state_defaults
                .entry(property_name.to_owned())
                .or_insert_with(|| borrowed_json_to_value(default));
        }
    }

    fn parse_sources(&mut self, value: &Json<'_>) {
        let Some(object) = value.as_object() else {
            log::warn!(target: log_target::PARSE_STYLE, "sources must be an object");
            return;
        };

        for (id, property) in members(object) {
            let mut error = Error::default();
            let Some(source) = convert_source(&Convertible::from(property), &mut error, id, &self.registry) else {
                log::warn!(target: log_target::PARSE_STYLE, "{}", error.message);
                continue;
            };

            self.sources.push(source);
        }
    }

    fn parse_sprites(&mut self, value: &Json<'_>) {
        if let Some(url) = value.as_str() {
            let sprite = Sprite::new("default", url);
            self.sprites.push(sprite);
        } else if let Some(array) = value.as_array() {
            let mut sprite_ids = HashSet::default();
            for sprite_value in array {
                if !sprite_value.is_object() {
                    log::warn!(target: log_target::PARSE_STYLE, "sprite child must be an object");
                    continue;
                }

                let mut error = Error::default();
                let Some(sprite) = convert::<Sprite>(&Convertible::from(sprite_value), &mut error) else {
                    log::warn!(target: log_target::PARSE_STYLE, "{}", error.message);
                    continue;
                };

                if sprite_ids.contains(&sprite.id) {
                    log::warn!(target: log_target::PARSE_STYLE, "sprite ids must be unique");
                    continue;
                }

                sprite_ids.insert(sprite.id.clone());
                self.sprites.push(sprite);
            }
        } else {
            log::warn!(target: log_target::PARSE_STYLE, "sprite must be an object or string");
        }
    }

    fn parse_layers(&mut self, value: &Json<'_>) {
        let mut ids: Vec<String> = Vec::new();

        let Some(array) = value.as_array() else {
            log::warn!(target: log_target::PARSE_STYLE, "layers must be an array");
            return;
        };

        let mut layers_map: HashMap<String, LayerEntry<'_>> = HashMap::default();

        for layer_value in array {
            let Some(layer_object) = layer_value.as_object() else {
                log::warn!(target: log_target::PARSE_STYLE, "layer must be an object");
                continue;
            };

            let Some(id) = member(layer_object, "id") else {
                log::warn!(target: log_target::PARSE_STYLE, "layer must have an id");
                continue;
            };

            let Some(id) = id.as_str() else {
                log::warn!(target: log_target::PARSE_STYLE, "layer id must be a string");
                continue;
            };

            let layer_id = id.to_owned();
            if layers_map.contains_key(&layer_id) {
                log::warn!(target: log_target::PARSE_STYLE, "duplicate layer id {layer_id}");
                continue;
            }

            layers_map.insert(layer_id.clone(), LayerEntry { value: layer_value, layer: None });
            ids.push(layer_id);
        }

        // Store a stack of layer IDs we're parsing right now. This is to
        // prevent reference cycles.
        let mut stack: Vec<String> = Vec::new();

        for id in &ids {
            if layers_map.contains_key(id) {
                parse_layer(&self.registry, id, &mut layers_map, &mut stack);
            }
        }

        for id in &ids {
            if let Some(entry) = layers_map.get_mut(id)
                && let Some(layer) = entry.layer.take()
            {
                self.layers.push(layer);
            }
        }
    }

    /// Statically evaluate layer properties to determine what font stacks are used.
    #[must_use]
    pub fn font_stacks(&self) -> VecSet<FontStack> {
        crate::layer::font_stacks(self.layers.iter().map(Layer::style))
    }
}

struct LayerEntry<'a> {
    value: &'a Json<'a>,
    layer: Option<Layer>,
}

fn parse_layer(
    registry: &StyleRegistry,
    id: &str,
    layers_map: &mut HashMap<String, LayerEntry<'_>>,
    stack: &mut Vec<String>,
) {
    let Some(entry) = layers_map.get(id) else {
        return;
    };
    if entry.layer.is_some() {
        // Skip parsing this again. We already have a valid layer definition.
        return;
    }
    let value = entry.value;

    // Make sure we have not previously attempted to parse this layer.
    if stack.iter().any(|s| s == id) {
        log::warn!(target: log_target::PARSE_STYLE, "layer reference of '{id}' is circular");
        return;
    }

    if let Some(ref_val) = value.as_object().and_then(|object| member(object, "ref")) {
        // This layer is referencing another layer. Recursively parse that layer.
        let Some(ref_) = ref_val.as_str() else {
            log::warn!(target: log_target::PARSE_STYLE, "layer ref of '{id}' must be a string");
            return;
        };

        if !layers_map.contains_key(ref_) {
            log::warn!(target: log_target::PARSE_STYLE, "layer '{id}' references unknown layer {ref_}");
            return;
        }

        // Recursively parse the referenced layer.
        stack.insert(0, id.to_owned());
        parse_layer(registry, ref_, layers_map, stack);
        stack.remove(0);

        let Some(reference) = layers_map.get(ref_).and_then(|e| e.layer.as_ref()) else {
            return;
        };

        let mut layer = reference.clone_ref(id);
        if let Some(error) = set_paint_properties(&mut layer, &Convertible::from(value), InvalidProperty::Skip) {
            log::warn!(target: log_target::PARSE_STYLE, "{}", error.message);
        }
        if let Some(entry) = layers_map.get_mut(id) {
            entry.layer = Some(layer);
        }
    } else {
        let mut error = Error::default();
        let Some(converted) =
            convert_layer_with(registry, &Convertible::from(value), &mut error, InvalidProperty::Skip)
        else {
            log::warn!(target: log_target::PARSE_STYLE, "{}", error.message);
            return;
        };
        if let Some(entry) = layers_map.get_mut(id) {
            entry.layer = Some(converted);
        }
    }
}

/// The message of a JSON syntax error: `serde_json`'s, instead of rapidjson's
/// (see the file comment).
fn format_json_parse_error(error: &serde_json::Error) -> String {
    error.to_string()
}

/// Parses a font file of a font face: an object with a `url`, and with an
/// optional `unicode-range` that limits the code points the font draws. Adds a
/// warning for each range it can't read, and skips the range.
fn parse_font_file(face_name: &str, font_file: &JsonObject<'_>, warnings: &mut Vec<String>) -> FontFace {
    let mut font_face = FontFace { name: face_name.to_owned(), url: String::new(), ranges: vec![ALL_CODE_POINTS] };

    if let Some(url) = member(font_file, "url").and_then(Json::as_str) {
        font_face.url = url.to_owned();
    }

    if let Some(unicode_range) = member(font_file, "unicode-range") {
        font_face.ranges.clear();
        if let Some(ranges) = unicode_range.as_array() {
            for range in ranges {
                let Some(range_string) = range.as_str() else {
                    continue;
                };
                if range_string.len() <= 2 {
                    continue;
                }
                // `substr` works on bytes: a multibyte character may be cut.
                let range_string = &range_string.as_bytes()[2..];
                let Some(pos) = range_string.iter().position(|&b| b == b'-') else {
                    continue;
                };
                let (start, end) = (&range_string[..pos], &range_string[pos + 1..]);
                if start.is_empty() || end.is_empty() {
                    continue;
                }
                // `util::parse<int>(s, 16)`: std::from_chars, the whole
                // string must be consumed.
                let parse = |s: &[u8]| std::str::from_utf8(s).ok().and_then(|s| i32::from_str_radix(s, 16).ok());
                match (parse(start), parse(end)) {
                    (Some(start), Some(end)) => font_face.ranges.push((start as u32, end as u32)),
                    _ => warnings
                        .push(format!("Invalid unicode-range in font-face: {}", String::from_utf8_lossy(range_string))),
                }
            }
        }
    }

    font_face
}

/// Parses fallback font faces from `json`, an array of font files in the format
/// of a font face in a style's `font-faces`, without a font name. It fails with
/// [`ErrorKind::InvalidArgument`](crate::error::ErrorKind) if `json` isn't such an
/// array.
pub fn parse_fallback_font_faces(json: &str) -> Result<FontFaces, crate::error::Error> {
    let document: Json<'_> = serde_json::from_str(json)
        .map_err(|error| crate::error::Error::invalid_argument(format_json_parse_error(&error)))?;
    let Some(files) = document.as_array() else {
        return Err(crate::error::Error::invalid_argument("fallback font faces must be an array"));
    };

    let mut font_faces = FontFaces::new();
    for font_file in files {
        let Some(font_file) = font_file.as_object() else {
            return Err(crate::error::Error::invalid_argument("each fallback font face must be an object"));
        };
        let mut warnings = Vec::new();
        let font_face = parse_font_file("", font_file, &mut warnings);
        if let Some(warning) = warnings.into_iter().next() {
            return Err(crate::error::Error::invalid_argument(warning));
        }
        if font_face.url.is_empty() {
            return Err(crate::error::Error::invalid_argument("each fallback font face must have a url"));
        }
        if font_face.ranges.is_empty() {
            return Err(crate::error::Error::invalid_argument("each fallback font face must cover a unicode-range"));
        }
        font_faces.push(font_face);
    }
    Ok(font_faces)
}

/// The "font-faces" block of `Parser::parse` (`MLN_TEXT_SHAPING_HARFBUZZ`).
fn parse_font_faces(faces: &JsonObject<'_>) -> FontFaces {
    let mut font_faces = FontFaces::new();
    for (face_name, face_value) in members(faces) {
        if let Some(files) = face_value.as_array() {
            // If the face is an array, we assume it is a list of font file
            // objects.
            for font_file in files {
                let Some(font_file) = font_file.as_object() else {
                    continue;
                };
                let mut warnings = Vec::new();
                let font_face = parse_font_file(face_name, font_file, &mut warnings);
                for warning in warnings {
                    log::warn!(target: log_target::PARSE_STYLE, "{warning}");
                }

                if !font_face.name.is_empty() && !font_face.url.is_empty() && !font_face.ranges.is_empty() {
                    font_faces.push(font_face);
                } else {
                    log::warn!(target: log_target::PARSE_STYLE, "Invalid font-face definition for: {face_name}");
                }
            }
        } else if let Some(url) = face_value.as_str() {
            font_faces.push(FontFace {
                name: face_name.to_owned(),
                url: url.to_owned(),
                ranges: vec![ALL_CODE_POINTS],
            });
        } else {
            log::warn!(target: log_target::PARSE_STYLE, "font-face must be an object or array");
        }
    }
    font_faces
}

/// The range of a font face without a `unicode-range`: every code point.
const ALL_CODE_POINTS: FontFaceRange = (0, 0x0010_FFFF);
