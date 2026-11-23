// The styles the app offers, as the web demo does (examples/web/web/styles.ts):
// keyless styles from two providers, which draw two tile schemas
// (OpenMapTiles and Shortbread). And the projections it draws them with.

use mln_util::error::Error;

/// A style the app can show.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Style {
    pub name: String,
    pub provider: String,
    pub url: String,
    /// Dark: the controls turn dark with it.
    pub dark: bool,
}

const STYLES: [(&str, &str, &str, bool); 6] = [
    ("Liberty", "OpenFreeMap", "https://tiles.openfreemap.org/styles/liberty", false),
    ("Bright", "OpenFreeMap", "https://tiles.openfreemap.org/styles/bright", false),
    ("Positron", "OpenFreeMap", "https://tiles.openfreemap.org/styles/positron", false),
    ("Dark Matter", "OpenFreeMap", "https://tiles.openfreemap.org/styles/dark", true),
    ("Fiord", "OpenFreeMap", "https://tiles.openfreemap.org/styles/fiord", true),
    ("Colorful", "VersaTiles", "https://tiles.versatiles.org/assets/styles/colorful/style.json", false),
];

/// The styles of the picker, Liberty first.
pub(crate) fn styles() -> Vec<Style> {
    STYLES
        .iter()
        .map(|&(name, provider, url, dark)| Style {
            name: name.to_owned(),
            provider: provider.to_owned(),
            url: url.to_owned(),
            dark,
        })
        .collect()
}

/// The style at `url`: one of the picker's, or another, which the app names
/// by the style's own name once it loads (`name` is empty until then).
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
