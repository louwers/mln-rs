// Ports the URL normalization half of src/mln/util/mapbox.hpp and
// src/mln/util/mapbox.cpp (`isCanonicalURL`, `makeQueryString`,
// `normalize{Source,Style,Sprite,Glyphs,Tile}URL`), which the online file
// source applies to every request.
//
// The canonicalize* half, used for tilesets and offline cache keys, is
// mln_storage_offline::mapbox: it needs the `regex` crate.

use crate::logging::{Event, Log};
use crate::tile_server_options::TileServerOptions;
use crate::url::{URL, transform_url_with};

/// `isCanonicalURL`
pub fn is_canonical_url(tile_server_options: &TileServerOptions, url: &str) -> bool {
    if tile_server_options.uri_scheme_alias().is_empty() || tile_server_options.base_url().is_empty() {
        return false;
    }
    let protocol = format!("{}://", tile_server_options.uri_scheme_alias());
    url.starts_with(&protocol)
}

/// `makeQueryString`
pub fn make_query_string(tile_server_options: &TileServerOptions, api_key: &str) -> String {
    if !tile_server_options.requires_api_key()
        || tile_server_options.api_key_parameter_name().is_empty()
        || api_key.is_empty()
    {
        String::new()
    } else {
        format!("?{}={}", tile_server_options.api_key_parameter_name(), api_key)
    }
}

/// `equals(str, segment, ref)`: `str.compare(segment.first, segment.second, ref) == 0`.
fn equals(s: &str, segment: (usize, usize), r: &str) -> bool {
    let start = segment.0.min(s.len());
    let end = start.saturating_add(segment.1).min(s.len());
    s.as_bytes()[start..end] == *r.as_bytes()
}

fn normalize(
    tile_server_options: &TileServerOptions,
    s: &str,
    api_key: &str,
    domain_name: &str,
    version_prefix: &Option<String>,
    template: &str,
    invalid: &str,
) -> String {
    let url = URL::new(s);
    if !equals(s, url.domain, domain_name) {
        Log::error(Event::ParseStyle, invalid);
        return s.to_owned();
    }
    let tpl = format!(
        "{}{}{}{}",
        tile_server_options.base_url(),
        version_prefix.as_deref().unwrap_or(""),
        template,
        make_query_string(tile_server_options, api_key)
    );
    transform_url_with(&tpl, s, &url)
}

/// `normalizeSourceURL`. C++ throws "You must provide API key for tile
/// sources" for a canonical URL without a required API key; that is the
/// `Err`.
pub fn normalize_source_url(tile_server_options: &TileServerOptions, s: &str, api_key: &str) -> Result<String, String> {
    if !is_canonical_url(tile_server_options, s) {
        return Ok(s.to_owned());
    }
    if tile_server_options.requires_api_key() && api_key.is_empty() {
        return Err("You must provide API key for tile sources".to_owned());
    }

    let suffix = if tile_server_options.uri_scheme_alias() == "mapbox" { "&secure" } else { "" };
    let url = URL::new(s);
    let domain_name = tile_server_options.source_domain_name();
    if !domain_name.is_empty() && !equals(s, url.domain, domain_name) {
        Log::error(Event::ParseStyle, "Invalid source URL");
        return Ok(s.to_owned());
    }

    let tpl = format!(
        "{}{}{}{}{}",
        tile_server_options.base_url(),
        tile_server_options.source_version_prefix().as_deref().unwrap_or(""),
        tile_server_options.source_template(),
        make_query_string(tile_server_options, api_key),
        suffix
    );
    Ok(transform_url_with(&tpl, s, &url))
}

/// `normalizeStyleURL`
pub fn normalize_style_url(tile_server_options: &TileServerOptions, s: &str, api_key: &str) -> String {
    if !is_canonical_url(tile_server_options, s) {
        return s.to_owned();
    }
    normalize(
        tile_server_options,
        s,
        api_key,
        tile_server_options.style_domain_name(),
        tile_server_options.style_version_prefix(),
        tile_server_options.style_template(),
        "Invalid style URL",
    )
}

/// `normalizeSpriteURL`
pub fn normalize_sprite_url(tile_server_options: &TileServerOptions, s: &str, api_key: &str) -> String {
    if !is_canonical_url(tile_server_options, s) {
        return s.to_owned();
    }
    normalize(
        tile_server_options,
        s,
        api_key,
        tile_server_options.sprites_domain_name(),
        tile_server_options.sprites_version_prefix(),
        tile_server_options.sprites_template(),
        "Invalid sprite URL",
    )
}

/// `normalizeGlyphsURL`
pub fn normalize_glyphs_url(tile_server_options: &TileServerOptions, s: &str, api_key: &str) -> String {
    if !is_canonical_url(tile_server_options, s) {
        return s.to_owned();
    }
    normalize(
        tile_server_options,
        s,
        api_key,
        tile_server_options.glyphs_domain_name(),
        tile_server_options.glyphs_version_prefix(),
        tile_server_options.glyphs_template(),
        "Invalid glyph URL",
    )
}

/// `normalizeTileURL`
pub fn normalize_tile_url(tile_server_options: &TileServerOptions, s: &str, api_key: &str) -> String {
    if !is_canonical_url(tile_server_options, s) {
        return s.to_owned();
    }
    normalize(
        tile_server_options,
        s,
        api_key,
        tile_server_options.tile_domain_name(),
        tile_server_options.tile_version_prefix(),
        tile_server_options.tile_template(),
        "Invalid tile URL",
    )
}

#[cfg(test)]
mod tests {
    // Ports cases of test/util/mapbox.test.cpp (MapLibre and MapTiler
    // configurations).
    use super::*;

    #[test]
    fn non_canonical_urls_pass_through() {
        let options = TileServerOptions::map_tiler_configuration();
        let url = "https://tiles.openfreemap.org/styles/liberty";
        assert_eq!(normalize_style_url(&options, url, ""), url);
        assert_eq!(normalize_source_url(&options, url, "").unwrap(), url);
        assert_eq!(normalize_tile_url(&options, url, "key"), url);
    }

    #[test]
    fn maptiler_style() {
        let options = TileServerOptions::map_tiler_configuration();
        assert_eq!(
            normalize_style_url(&options, "maptiler://maps/streets", "abc"),
            "https://api.maptiler.com/maps/streets/style.json?key=abc"
        );
        assert_eq!(
            normalize_source_url(&options, "maptiler://sources/v3", "abc").unwrap(),
            "https://api.maptiler.com/tiles/v3/tiles.json?key=abc"
        );
        assert!(normalize_source_url(&options, "maptiler://sources/v3", "").is_err());
    }

    #[test]
    fn maplibre_style() {
        let options = TileServerOptions::maplibre_configuration();
        assert_eq!(
            normalize_style_url(&options, "maplibre://maps/style", ""),
            "https://demotiles.maplibre.org/style.json"
        );
    }
}
