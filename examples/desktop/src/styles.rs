// The styles the app offers, as the web demo does (examples/web/web/styles.ts):
// keyless styles from two providers, which draw two tile schemas
// (OpenMapTiles and Shortbread).

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
