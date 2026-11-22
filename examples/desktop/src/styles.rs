// The styles the app offers, as the web demo does (examples/web/web/styles.ts):
// keyless styles from three providers, which draw two tile schemas
// (OpenMapTiles and Shortbread), one of them with Mapterhorn's terrain added.

use mln_util::error::Error;

/// A style the app can show.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Style {
    pub name: String,
    pub provider: String,
    pub url: String,
    /// Dark: the controls turn dark with it.
    pub dark: bool,
    /// The terrain the app adds to the style once it loads.
    pub terrain: Option<Terrain>,
}

/// 3D terrain that the app adds to a style that has none: a DEM source, the
/// style's `terrain` on it, a hillshade of it, and a `sky` where the style
/// has none, so that fog shows at a high pitch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Terrain {
    /// The `raster-dem` source's JSON.
    pub source: &'static str,
    pub exaggeration: f64,
    /// The layer that the hillshade goes under, if the style has it:
    /// shading the ground, but not what stands on it or the labels.
    pub hillshade_before: &'static str,
}

/// Mapterhorn's global Terrarium tiles (mapterhorn.com/data-access), whose
/// TileJSON brings their attribution.
const MAPTERHORN: Terrain = Terrain {
    source: r#"{"type":"raster-dem","url":"https://tiles.mapterhorn.com/tilejson.json"}"#,
    exaggeration: 1.3,
    hillshade_before: "building",
};

const LIBERTY: &str = "https://tiles.openfreemap.org/styles/liberty";

const STYLES: [(&str, &str, &str, bool, Option<Terrain>); 7] = [
    ("Liberty", "OpenFreeMap", LIBERTY, false, None),
    ("Liberty 3D", "OpenFreeMap + Mapterhorn", LIBERTY, false, Some(MAPTERHORN)),
    ("Bright", "OpenFreeMap", "https://tiles.openfreemap.org/styles/bright", false, None),
    ("Positron", "OpenFreeMap", "https://tiles.openfreemap.org/styles/positron", false, None),
    ("Dark Matter", "OpenFreeMap", "https://tiles.openfreemap.org/styles/dark", true, None),
    ("Fiord", "OpenFreeMap", "https://tiles.openfreemap.org/styles/fiord", true, None),
    ("Colorful", "VersaTiles", "https://tiles.versatiles.org/assets/styles/colorful/style.json", false, None),
];

/// The styles of the picker, Liberty first.
pub(crate) fn styles() -> Vec<Style> {
    STYLES
        .iter()
        .map(|&(name, provider, url, dark, terrain)| Style {
            name: name.to_owned(),
            provider: provider.to_owned(),
            url: url.to_owned(),
            dark,
            terrain,
        })
        .collect()
}

/// The style at `url`: the first of the picker's there, or another, which
/// the app names by the style's own name once it loads (`name` is empty
/// until then).
pub(crate) fn style_for(url: &str) -> Style {
    styles().into_iter().find(|style| style.url == url).unwrap_or_else(|| Style {
        name: String::new(),
        provider: url
            .split("://")
            .nth(1)
            .and_then(|rest| rest.split('/').next())
            .filter(|host| !host.is_empty())
            .unwrap_or("local file")
            .to_owned(),
        url: url.to_owned(),
        dark: false,
        terrain: None,
    })
}

/// The sky that [`add_terrain`] gives a style without one: the style spec's
/// default colours, whose white fog greys the far ground.
const SKY: &str = r##"{"sky-color":"#88c6fc","horizon-color":"#ffffff","fog-color":"#ffffff","fog-ground-blend":0.5}"##;

/// The ID of the source that [`add_terrain`] adds.
pub(crate) const TERRAIN_SOURCE: &str = "terrain";

/// Adds `terrain` to the style that `map` loaded, unless it has the source
/// already. Fails while no style is loaded.
pub(crate) fn add_terrain(map: &mut mln_map::Map, terrain: &Terrain) -> Result<(), Error> {
    if map.source(TERRAIN_SOURCE).is_some() {
        return Ok(());
    }
    map.add_source_json(TERRAIN_SOURCE, terrain.source)?;
    map.edit_loaded_style(|style| {
        let hillshade = format!(
            r##"{{"id":"terrain-hillshade","type":"hillshade","source":"{TERRAIN_SOURCE}","paint":{{"hillshade-exaggeration":0.3,"hillshade-shadow-color":"#473b24"}}}}"##
        );
        let before = style.get_layer(terrain.hillshade_before).map(|_| terrain.hillshade_before);
        style.add_layer_json(&hillshade, before)?;
        let root = format!(r#"{{"source":"{TERRAIN_SOURCE}","exaggeration":{}}}"#, terrain.exaggeration);
        style.set_root_property_json("terrain", &root)?;
        if style.get_sky().is_none() {
            style.set_root_property_json("sky", SKY)?;
        }
        Ok(())
    })
}

/// The projection the app draws the map with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Projection {
    /// The style's own.
    #[default]
    Style,
    /// Web Mercator.
    Flat,
    /// The sphere at every zoom: `vertical-perspective`.
    Globe,
    /// The sphere, handing over to Web Mercator between zoom 10 and 12: the
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
            Projection::Handoff => "The sphere, handing over to Web Mercator between zoom 10 and 12 (globe)",
        }
    }

    /// Sets the style's `projection` to this one: for `Style`, back to the
    /// style document's own. Does nothing while no style is loaded; the app
    /// sets it again once one loads.
    pub(crate) fn apply(self, map: &mut mln_map::Map) -> Result<(), Error> {
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
