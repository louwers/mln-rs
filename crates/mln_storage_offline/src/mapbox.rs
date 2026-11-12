// Ports the canonicalize* half of src/mln/util/mapbox.cpp
// (`canonicalizeTileURL`, `canonicalizeTileset`, `canonicalize{Source,
// Sprite,Glyph}URL` and their helpers `createTokenMap`,
// `isNormalizedSourceURL`, `isNormalizedURL`); the normalize* half is
// mln_util::mapbox. Here because it needs the `regex` crate (C++
// std::regex, ECMAScript syntax: the patterns are built from the tile server
// templates the same way and mean the same in both).

use mln_style_types::SourceType;
use mln_util::constants::TILE_SIZE_I;
use mln_util::mapbox::is_canonical_url;
use mln_util::tile_server_options::TileServerOptions;
use mln_util::tileset::Tileset;
use mln_util::url::{Path, URL};
use mln_vec_map::VecMap;
use regex::Regex;

fn ends_with(s: &str, search: &str) -> bool {
    s.ends_with(search)
}

/// `nthOccurrenceFromEnd(str, search, nth)`.
fn nth_occurrence_from_end(s: &str, search: &str, nth: i32) -> i32 {
    let mut pos = s.len();
    let mut cnt = 0;

    while cnt != nth {
        // `pos -= 1; pos = str.rfind(search, pos)`: a match starting at or
        // before `pos`.
        if pos == 0 {
            return -1;
        }
        pos -= 1;
        let end = (pos + search.len()).min(s.len());
        match s[..end].rfind(search) {
            Some(p) => pos = p,
            None => return -1,
        }
        cnt += 1;
    }
    pos as i32
}

fn replace(s: &mut String, search: &str, replacement: &str) {
    *s = s.replace(search, replacement);
}

fn substr(s: &str, segment: (usize, usize)) -> &str {
    let start = segment.0.min(s.len());
    let end = start.saturating_add(segment.1).min(s.len());
    &s[start..end]
}

/// `std::regex_replace(urlTemplate, tokenPattern, "(.+)", match_any)`.
fn replace_tokens(template: &str, token_pattern: &str) -> String {
    Regex::new(token_pattern).expect("token pattern").replace_all(template, "(.+)").into_owned()
}

/// `createTokenMap(urlTemplate, url)`.
fn create_token_map(url_template: &str, url: &str) -> VecMap<String, String> {
    let parsed_url = URL::new(url);

    // e.g. tiles/satellite/tiles.json
    let path = substr(url, parsed_url.path);

    // --- find tokens in the urlTemplate (such as {path}, ...)
    let r = Regex::new(r"\{(domain|path|directory|filename|extension)\}").expect("token regex");
    let mut token_list: Vec<(String, String)> = Vec::new();
    for m in r.captures_iter(url_template) {
        if let Some(group) = m.get(1)
            && !group.as_str().is_empty()
        {
            token_list.push((group.as_str().to_owned(), String::new()));
        }
    }

    // --- extract token values
    // /tiles{path}/tiles.json in tiles/satellite/tiles.json
    // e.g. path = /satellite
    let template_pattern =
        replace_tokens(url_template, r"\{domain\}|\{path\}|\{directory\}|\{filename\}|\{extension\}");
    if let Ok(r2) = Regex::new(&template_pattern) {
        let mut idx = 0;
        for m in r2.captures_iter(path) {
            if let Some(group) = m.get(1)
                && !group.as_str().is_empty()
            {
                // C++ indexes `tokenList[idx]` without a bounds check.
                if let Some(token) = token_list.get_mut(idx) {
                    token.1 = group.as_str().to_owned();
                }
                idx += 1;
            }
        }
    }

    // `std::copy` into a `std::map`: the first of equal keys wins.
    let mut token_map = VecMap::new();
    for (key, value) in token_list {
        token_map.entry(key).or_insert(value);
    }
    token_map
}

/// `isNormalizedSourceURL(baseURL, urlTemplate, url)`.
fn is_normalized_source_url(base_url: &str, url_template: &str, url: &str) -> bool {
    if base_url.is_empty() || url_template.is_empty() {
        return false;
    }

    let url_pattern = replace_tokens(url_template, r"\{.+\}") + "(.*)";
    let pattern = format!("^(?:{base_url}{url_pattern})$");

    // `std::regex_match`: the whole string.
    Regex::new(&pattern).is_ok_and(|r| r.is_match(url))
}

/// `isNormalizedURL(tileServerOptions, str)`.
fn is_normalized_url(tile_server_options: &TileServerOptions, s: &str) -> bool {
    let url = URL::new(s);
    let path = Path::with_range(s, url.path.0, url.path.1);

    // Make sure that we are dealing with a valid tile URL.
    // Has to be with the tile template prefix, with a valid filename + extension

    if tile_server_options.uri_scheme_alias() == "mapbox" {
        let base_url = URL::new(tile_server_options.base_url());
        let mut domain = substr(s, url.domain).to_owned();

        replace(&mut domain, ".cn", ".com"); // hack for mapbox china

        let mut ref_domain = substr(tile_server_options.base_url(), base_url.domain).to_owned();
        let ref_domain_idx = nth_occurrence_from_end(&ref_domain, ".", 2);
        let start = (ref_domain_idx + 1) as usize;
        ref_domain = ref_domain[start.min(ref_domain.len())..].to_owned();

        if !ends_with(&domain, &ref_domain) || path.filename.1 == 0 || path.extension.1 <= 1 {
            // Not a proper tile URL.
            return false;
        }
    } else if path.filename.1 == 0 || path.extension.1 <= 1 {
        // Not a proper tile URL.
        return false;
    }

    true
}

/// `canonicalizeTileURL(tileServerOptions, str, type, tileSize)`.
pub fn canonicalize_tile_url(
    tile_server_options: &TileServerOptions,
    s: &str,
    type_: SourceType,
    tile_size: u16,
) -> String {
    if !is_normalized_url(tile_server_options, s) {
        return s.to_owned();
    }

    let mut result = format!("{}://{}", tile_server_options.uri_scheme_alias(), tile_server_options.tile_domain_name());

    let url = URL::new(s);
    let path = Path::with_range(s, url.path.0, url.path.1);

    let version_prefix_len = tile_server_options.tile_version_prefix().as_ref().map_or(0, String::len);

    result += substr(s, (path.directory.0 + version_prefix_len, path.directory.1.wrapping_sub(version_prefix_len)));
    result += substr(s, path.filename);
    if type_ == SourceType::Raster || type_ == SourceType::RasterDEM {
        result += if tile_size == TILE_SIZE_I { "@2x" } else { "{ratio}" };
    }
    result += substr(s, path.extension);

    // Append the query string, minus the access token parameter.
    if url.query.1 > 1 {
        let mut idx = Some(url.query.0);
        let mut has_query = false;
        while let Some(i) = idx {
            let i = i + 1; // skip & or ?
            let ampersand_idx = s[i.min(s.len())..].find('&').map(|p| p + i);
            let api_key = format!("{}=", tile_server_options.api_key_parameter_name());
            if !s[i.min(s.len())..].starts_with(&api_key) {
                result.push(if has_query { '&' } else { '?' });
                let end = ampersand_idx.unwrap_or(s.len());
                result += &s[i.min(s.len())..end];
                has_query = true;
            }
            idx = ampersand_idx;
        }
    }

    result
}

/// `canonicalizeTileset(tileServerOptions, tileset, sourceURL, type, tileSize)`.
pub fn canonicalize_tileset(
    tile_server_options: &TileServerOptions,
    tileset: &mut Tileset,
    source_url: &str,
    type_: SourceType,
    tile_size: u16,
) {
    if is_canonical_url(tile_server_options, source_url)
        || is_normalized_source_url(tile_server_options.base_url(), tile_server_options.source_template(), source_url)
    {
        for url in &mut tileset.tiles {
            *url = canonicalize_tile_url(tile_server_options, url, type_, tile_size);
        }
    }
}

/// The shared body of `canonicalize{Source,Sprite,Glyph}URL`.
fn canonicalize(
    tile_server_options: &TileServerOptions,
    url: &str,
    template: &str,
    domain_name: &str,
    version_prefix: Option<&str>,
) -> String {
    if url.is_empty() {
        return url.to_owned();
    }

    if is_normalized_source_url(tile_server_options.base_url(), template, url) {
        let token_map = create_token_map(template, url);

        let mut ret_val = format!("{}://", tile_server_options.uri_scheme_alias());
        if let Some(domain) = token_map.get("domain") {
            ret_val += domain;
        } else if !domain_name.is_empty() {
            ret_val += domain_name;
        }

        if let Some(prefix) = version_prefix {
            ret_val += prefix;
        }

        if let Some(path) = token_map.get("path") {
            ret_val += path;
        } else {
            for token in ["directory", "filename", "extension"] {
                if let Some(value) = token_map.get(token) {
                    ret_val += value;
                }
            }
        }

        return ret_val;
    }
    url.to_owned()
}

/// `canonicalizeSourceURL(tileServerOptions, url)`.
#[must_use]
pub fn canonicalize_source_url(tile_server_options: &TileServerOptions, url: &str) -> String {
    canonicalize(
        tile_server_options,
        url,
        tile_server_options.source_template(),
        tile_server_options.source_domain_name(),
        tile_server_options.source_version_prefix().as_deref(),
    )
}

/// `canonicalizeSpriteURL(tileServerOptions, url)`.
#[must_use]
pub fn canonicalize_sprite_url(tile_server_options: &TileServerOptions, url: &str) -> String {
    canonicalize(
        tile_server_options,
        url,
        tile_server_options.sprites_template(),
        tile_server_options.sprites_domain_name(),
        tile_server_options.sprites_version_prefix().as_deref(),
    )
}

/// `canonicalizeGlyphURL(tileServerOptions, url)`.
#[must_use]
pub fn canonicalize_glyph_url(tile_server_options: &TileServerOptions, url: &str) -> String {
    // "https://api.maptiler.com/fonts/{fontstack}/{range}.pbf?key=uwvyvzaF2P7UWbyOEvjU"
    canonicalize(
        tile_server_options,
        url,
        tile_server_options.glyphs_template(),
        tile_server_options.glyphs_domain_name(),
        tile_server_options.glyphs_version_prefix().as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maptiler_urls() {
        let options = TileServerOptions::map_tiler_configuration();
        assert_eq!(
            "maptiler://sources/streets",
            canonicalize_source_url(&options, "https://api.maptiler.com/tiles/streets/tiles.json?key=abc")
        );
        assert_eq!(
            "maptiler://fonts/{fontstack}/{range}.pbf",
            canonicalize_glyph_url(&options, "https://api.maptiler.com/fonts/{fontstack}/{range}.pbf?key=abc")
        );
        assert_eq!("http://127.0.0.1:3000/sprite", canonicalize_sprite_url(&options, "http://127.0.0.1:3000/sprite"));
        assert_eq!(
            "maptiler://tiles/tiles/v3/{z}/{x}/{y}.pbf",
            canonicalize_tile_url(
                &options,
                "https://api.maptiler.com/tiles/v3/{z}/{x}/{y}.pbf?key=abc",
                SourceType::Vector,
                512
            )
        );
        assert_eq!(3, nth_occurrence_from_end("api.maptiler.com", ".", 2));
    }
}
