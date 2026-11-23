// The styles the app offers, from the shared catalog (examples/styles.json)
// that the web demo reads too. And the projections it draws them with.

use mln_util::error::Error;

const STYLES_JSON: &str = include_str!("../../styles.json");

/// Where a style loads from: a URL or a style document's JSON text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StyleSource {
    Url(String),
    Json(String),
}

/// A style the app can show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Style {
    /// Names the style on the command line (`--style <key>`); `None` for a
    /// custom URL or path.
    pub key: Option<String>,
    pub name: String,
    pub provider: String,
    pub source: StyleSource,
    /// Dark: the controls turn dark with it.
    pub dark: bool,
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
            Some(Style { key: Some(key), name, provider, source, dark })
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
        })
}

/// The style named by `spec`: a key or URL from `examples/styles.json`, or
/// another URL or local file path, which the app names by the style's own name
/// once it loads (`name` is empty until then).
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
    Style { key: None, name: String::new(), provider, source: StyleSource::Url(url), dark: false }
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

#[cfg(test)]
mod tests {
    use mln_util::hash::HashSet;

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
    }
}
