// The styles the app offers, from the shared catalog (examples/styles.json)
// that the web demo reads too, the terrain it adds to some of them, and where
// some of them open. And the projections it draws them with.

use mln_engine::camera::CameraOptions;
use mln_style::error::Error;
use mln_style::geo::LatLng;

const STYLES_JSON: &str = include_str!("../../styles.json");

/// Where a style loads from: a URL or a style document's JSON text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StyleSource {
    Url(String),
    Json(String),
}

/// A style the app can show.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Style {
    /// Names the style on the command line (`--style <key>`); `None` for a
    /// custom URL or path.
    pub key: Option<String>,
    pub name: String,
    pub provider: String,
    pub source: StyleSource,
    /// Dark: the controls turn dark with it.
    pub dark: bool,
    /// The terrain the app adds to the style once it loads.
    pub terrain: Option<Terrain>,
    /// Where the camera goes when the app opens the style or the picker
    /// picks it; without one, it stays where it is.
    pub camera: Option<Camera>,
}

/// A catalog entry's `camera`: a center and a zoom. The bearing and pitch
/// stay as they are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Camera {
    pub center: LatLng,
    pub zoom: f64,
}

impl Camera {
    /// A catalog entry's `camera`, whose `center` is a longitude and a
    /// latitude, as a style's is; `None` where it lacks a part.
    fn parse(entry: &serde_json::Value) -> Option<Camera> {
        let [longitude, latitude] = entry.get("center")?.as_array()?.as_slice() else {
            return None;
        };
        Some(Camera {
            center: LatLng::new(latitude.as_f64()?, longitude.as_f64()?).ok()?,
            zoom: entry.get("zoom")?.as_f64()?,
        })
    }

    /// The camera options that move the camera here.
    pub(crate) fn options(self) -> CameraOptions {
        CameraOptions { center: Some(self.center), zoom: Some(self.zoom), ..CameraOptions::default() }
    }
}

/// 3D terrain that the app adds to a style that has none, from a catalog
/// entry's `terrain`, as the web demo does (examples/web/web/styles.ts): a
/// `raster-dem` source, the style's `terrain` on it, and optionally a
/// hillshade of it and a sky for a style that has none.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Terrain {
    /// The `raster-dem` source's JSON.
    pub source: String,
    pub exaggeration: f64,
    pub hillshade: Option<Hillshade>,
    /// The style's `sky` where it has none, as JSON.
    pub sky: Option<String>,
}

/// A hillshade layer of a [`Terrain`]'s source.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hillshade {
    /// The layer's paint, as JSON.
    pub paint: String,
    /// The layer that the hillshade goes under, if the style has it, such as
    /// the first that stands on the ground; else it goes on top.
    pub before: Option<String>,
}

impl Terrain {
    /// A catalog entry's `terrain`, or `None` where it lacks a part.
    fn parse(entry: &serde_json::Value) -> Option<Terrain> {
        let object = |value: &serde_json::Value, key: &str| {
            value.get(key).filter(|value| value.is_object()).map(ToString::to_string)
        };
        let hillshade = match entry.get("hillshade") {
            Some(hillshade) => Some(Hillshade {
                paint: object(hillshade, "paint").unwrap_or_else(|| "{}".to_owned()),
                before: match hillshade.get("before") {
                    Some(before) => Some(before.as_str()?.to_owned()),
                    None => None,
                },
            }),
            None => None,
        };
        Some(Terrain {
            source: object(entry, "source")?,
            exaggeration: entry.get("exaggeration")?.as_f64()?,
            hillshade,
            sky: object(entry, "sky"),
        })
    }
}

fn parse_catalog() -> (String, Vec<Style>) {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(STYLES_JSON) else {
        return (String::new(), Vec::new());
    };
    let default = root.get("default").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    let styles = root
        .get("styles")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let key = entry.get("key")?.as_str()?.to_owned();
            let name = entry.get("name")?.as_str()?.to_owned();
            let provider = entry.get("provider")?.as_str()?.to_owned();
            let source = if let Some(url) = entry.get("url").and_then(serde_json::Value::as_str) {
                StyleSource::Url(url.to_owned())
            } else {
                StyleSource::Json(entry.get("json")?.to_string())
            };
            let dark = entry.get("dark").and_then(serde_json::Value::as_bool).unwrap_or(false);
            let terrain = match entry.get("terrain") {
                Some(terrain) => Some(Terrain::parse(terrain)?),
                None => None,
            };
            let camera = match entry.get("camera") {
                Some(camera) => Some(Camera::parse(camera)?),
                None => None,
            };
            Some(Style { key: Some(key), name, provider, source, dark, terrain, camera })
        })
        .collect();
    (default, styles)
}

/// The styles of the picker, in `examples/styles.json`'s order.
pub(crate) fn styles() -> Vec<Style> {
    parse_catalog().1
}

/// The default style from `examples/styles.json`.
pub(crate) fn default_style() -> Style {
    let (default_key, list) = parse_catalog();
    list.iter()
        .find(|style| style.key.as_deref() == Some(default_key.as_str()))
        .or_else(|| list.first())
        .cloned()
        .unwrap_or_else(|| Style {
            key: None,
            name: String::new(),
            provider: String::new(),
            source: StyleSource::Json("{}".to_owned()),
            dark: false,
            terrain: None,
            camera: None,
        })
}

/// The style named by `spec`: a key from `examples/styles.json`, the first
/// style there with the URL `spec`, or another URL or local file path, which
/// the app names by the style's own name once it loads (`name` is empty until
/// then).
pub(crate) fn style_for(spec: &str) -> Style {
    if let Some(known) = styles().into_iter().find(|style| {
        style.key.as_deref() == Some(spec) || matches!(&style.source, StyleSource::Url(url) if url == spec)
    }) {
        return known;
    }
    let url = if spec.contains("://") { spec.to_owned() } else { format!("file://{spec}") };
    let provider = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .filter(|host| !host.is_empty())
        .unwrap_or("local file")
        .to_owned();
    Style {
        key: None,
        name: String::new(),
        provider,
        source: StyleSource::Url(url),
        dark: false,
        terrain: None,
        camera: None,
    }
}

/// The ID of the source that [`add_terrain`] adds.
pub(crate) const TERRAIN_SOURCE: &str = "terrain";

/// The ID of the hillshade layer that [`add_terrain`] adds.
const TERRAIN_HILLSHADE: &str = "terrain-hillshade";

/// Adds `terrain` to the style that `map` loaded, unless it has the source
/// already: the source, the style's `terrain` on it, the hillshade, if any,
/// and the sky, if any and the style has none. Fails while no style is loaded.
pub(crate) fn add_terrain(map: &mut mln_engine::map::Map, terrain: &Terrain) -> Result<(), Error> {
    if map.source(TERRAIN_SOURCE).is_some() {
        return Ok(());
    }
    map.add_source_json(TERRAIN_SOURCE, &terrain.source)?;
    map.edit_loaded_style(|style| {
        if let Some(hillshade) = &terrain.hillshade {
            let layer = format!(
                r#"{{"id":"{TERRAIN_HILLSHADE}","type":"hillshade","source":"{TERRAIN_SOURCE}","paint":{}}}"#,
                hillshade.paint
            );
            let before = hillshade.before.as_deref().filter(|id| style.get_layer(id).is_some());
            style.add_layer_json(&layer, before)?;
        }
        let root = format!(r#"{{"source":"{TERRAIN_SOURCE}","exaggeration":{}}}"#, terrain.exaggeration);
        style.set_root_property_json("terrain", &root)?;
        match &terrain.sky {
            Some(sky) if style.get_sky().is_none() => style.set_root_property_json("sky", sky),
            _ => Ok(()),
        }
    })
}

/// The projection the app draws the map with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Projection {
    /// The style's own.
    #[default]
    Style,
    /// Web Mercator: `mercator`.
    Flat,
    /// The sphere at every zoom: `vertical-perspective`.
    Globe,
    /// The sphere, handing over to Web Mercator between zoom 11 and 12: the
    /// `globe` preset.
    Handoff,
}

impl Projection {
    pub(crate) const ALL: [Projection; 4] =
        [Projection::Style, Projection::Flat, Projection::Globe, Projection::Handoff];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Projection::Style => "Style",
            Projection::Flat => "Flat",
            Projection::Globe => "Globe",
            Projection::Handoff => "Handoff",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Projection::Style => "The projection the style declares",
            Projection::Flat => "Web Mercator at every zoom (mercator)",
            Projection::Globe => "The sphere at every zoom (vertical-perspective)",
            Projection::Handoff => "The sphere, handing over to Web Mercator between zoom 11 and 12 (globe)",
        }
    }

    /// Sets the style's `projection` to this one: for `Style`, back to the
    /// style document's own. Does nothing while no style is loaded; the app
    /// sets it again once one loads.
    pub(crate) fn apply(self, map: &mut mln_engine::map::Map) -> Result<(), Error> {
        if !map.style_document_loaded() {
            return Ok(());
        }
        let projection = match self {
            Projection::Style => serde_json::from_str::<serde_json::Value>(map.style().get_json())
                .ok()
                .and_then(|document| document.get("projection").cloned())
                .unwrap_or_default()
                .to_string(),
            Projection::Flat => r#"{"type":"mercator"}"#.to_owned(),
            Projection::Globe => r#"{"type":"vertical-perspective"}"#.to_owned(),
            Projection::Handoff => r#"{"type":"globe"}"#.to_owned(),
        };
        map.edit_loaded_style(|style| style.set_root_property_json("projection", &projection))
    }
}

#[cfg(test)]
mod tests {
    use mln_style::hash::HashSet;

    use super::*;

    #[test]
    fn shared_catalog_is_valid() {
        let catalog = styles();
        assert!(!catalog.is_empty(), "examples/styles.json must list at least one style");

        let mut keys = HashSet::default();
        for style in &catalog {
            let key = style.key.as_deref().expect("catalog styles have keys");
            assert_ne!(key, "");
            assert!(keys.insert(key), "duplicate style key: {key}");
            assert!(!style.name.is_empty(), "style {key} missing name");
            assert!(!style.provider.is_empty(), "style {key} missing provider");
            match &style.source {
                StyleSource::Url(url) => assert!(url.starts_with("https://"), "style {key} url: {url}"),
                StyleSource::Json(json) => {
                    let parsed: serde_json::Value =
                        serde_json::from_str(json).unwrap_or_else(|e| panic!("style {key} has invalid JSON: {e}"));
                    assert_eq!(parsed["version"], 8, "style {key} must be style spec v8");
                }
            }
        }

        let default = default_style();
        assert_eq!(default.key.as_deref(), Some("liberty"));
        assert_eq!(style_for("americana").key.as_deref(), Some("americana"));
        assert_eq!(style_for("osm-carto").key.as_deref(), Some("osm-carto"));
        assert!(matches!(style_for("osm-carto").source, StyleSource::Json(_)));
        assert_eq!(style_for("https://tiles.openfreemap.org/styles/liberty"), default);
        assert_eq!(style_for("/tmp/custom.json").source, StyleSource::Url("file:///tmp/custom.json".to_owned()));
        // Satellite 3D shares Satellite's URL, which opens the plain style.
        let satellite_3d = style_for("satellite-3d");
        assert!(satellite_3d.terrain.is_some(), "Satellite 3D's terrain");
        assert_eq!(satellite_3d.source, style_for("satellite").source);
        let StyleSource::Url(url) = &satellite_3d.source else { panic!("Satellite 3D has a URL") };
        assert_eq!(style_for(url).key.as_deref(), Some("satellite"));
        // Liberty 3D shades the ground under Liberty's buildings.
        let liberty_3d = style_for("liberty-3d").terrain.expect("Liberty 3D's terrain");
        assert_eq!(liberty_3d.hillshade.expect("Liberty 3D's hillshade").before.as_deref(), Some("building"));
        assert!(liberty_3d.sky.is_some(), "Liberty 3D's sky");
        // The Arctic opens over Greenland; Liberty keeps the camera.
        let arctic = Camera { center: LatLng::new(78.0, -40.0).unwrap(), zoom: 1.2 };
        assert_eq!(style_for("arctic").camera, Some(arctic));
        assert_eq!(default.camera, None);
    }

    #[test]
    fn camera_takes_a_center_and_a_zoom() {
        let parse = |json: &str| Camera::parse(&serde_json::from_str(json).unwrap());
        let camera = parse(r#"{"center":[-40,78],"zoom":1.2}"#).unwrap();
        assert_eq!(camera, Camera { center: LatLng::new(78.0, -40.0).unwrap(), zoom: 1.2 });
        assert_eq!(camera.options().center, Some(camera.center));
        assert_eq!(camera.options().zoom, Some(1.2));
        assert_eq!(camera.options().bearing, None);
        // A camera without both parts, or with a latitude past a pole, is none.
        assert_eq!(parse(r#"{"center":[-40,78]}"#), None);
        assert_eq!(parse(r#"{"center":[-40],"zoom":1}"#), None);
        assert_eq!(parse(r#"{"center":[0,91],"zoom":1}"#), None);
    }

    #[test]
    fn terrain_takes_an_optional_hillshade_and_sky() {
        let parse = |json: &str| Terrain::parse(&serde_json::from_str(json).unwrap());
        let source = r#""source":{"type":"raster-dem","url":"https://example.com/dem.json"}"#;
        let bare = parse(&format!(r#"{{{source},"exaggeration":1.5}}"#)).unwrap();
        assert_eq!(bare.exaggeration, 1.5);
        assert_eq!(bare.hillshade, None);
        assert_eq!(bare.sky, None);
        let shaded = parse(&format!(
            r#"{{{source},"exaggeration":1,"hillshade":{{"paint":{{"hillshade-exaggeration":0.3}},"before":"building"}}}}"#
        ))
        .unwrap();
        let hillshade = shaded.hillshade.unwrap();
        assert_eq!(hillshade.paint, r#"{"hillshade-exaggeration":0.3}"#);
        assert_eq!(hillshade.before.as_deref(), Some("building"));
        assert_eq!(
            parse(&format!(r#"{{{source},"exaggeration":1,"hillshade":{{}}}}"#)).unwrap().hillshade.unwrap().paint,
            "{}"
        );
        let sky = parse(&format!(r##"{{{source},"exaggeration":1,"sky":{{"fog-color":"#fff"}}}}"##)).unwrap().sky;
        assert_eq!(sky.as_deref(), Some(r##"{"fog-color":"#fff"}"##));
        // An entry without its source or exaggeration has no terrain.
        assert_eq!(parse(r#"{"exaggeration":1}"#), None);
        assert_eq!(parse(&format!("{{{source}}}")), None);
    }
}
