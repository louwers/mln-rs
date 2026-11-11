// Ports src/mln/util/mapbox.hpp and src/mln/util/mapbox.cpp: the
// normalize* half (`isCanonicalURL`, `makeQueryString`,
// `normalize{Source,Style,Sprite,Glyphs,Tile}URL`), which the online file
// source applies to every request, and the canonicalize* half
// (`canonicalizeTileURL`, `canonicalizeTileset`,
// `canonicalize{Source,Sprite,Glyph}URL` and their helpers `createTokenMap`,
// `isNormalizedSourceURL`, `isNormalizedURL`), used for tilesets and offline
// cache keys.
//
// C++ builds std::regex (ECMAScript) patterns from the tile server
// templates: literal text, where `.` matches any character, with tokens
// replaced by `(.+)`. The canonicalize* half matches that shape with its own
// backtracking matcher instead of a regex engine, which would add far more to
// the browser build.

use std::collections::BTreeMap;

use crate::constants::TILE_SIZE_I;
use crate::logging::{Event, Log};
use crate::tile_server_options::TileServerOptions;
use crate::tileset::Tileset;
use crate::url::{Path, URL, transform_url_with};

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

/// One element of a pattern built from a tile server template: the part of
/// ECMAScript regex syntax that the templates turn into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Piece {
    /// A literal character.
    Char(char),
    /// `.`
    Any,
    /// `(.+)`, captured.
    Plus,
    /// `(.*)`, not captured.
    Star,
}

/// One match of a pattern: its char range and the ranges of its `(.+)`s.
struct Match {
    start: usize,
    end: usize,
    captures: Vec<(usize, usize)>,
}

/// `.` matches any character but these (ECMAScript line terminators).
fn is_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

/// Appends `text` read as a regex of literal characters and `.`. `None` for
/// any other regex syntax: such a template has no pattern, as one std::regex
/// rejects or this port doesn't read.
fn push_literal(pieces: &mut Vec<Piece>, text: &[char]) -> Option<()> {
    for &c in text {
        match c {
            '.' => pieces.push(Piece::Any),
            '\\' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' => return None,
            c => pieces.push(Piece::Char(c)),
        }
    }
    Some(())
}

/// The pattern of `template` with each of `groups` (char ranges, in order)
/// replaced by `(.+)`, as `std::regex_replace(template, …, "(.+)")` builds it.
fn pattern_with_groups(template: &[char], groups: &[(usize, usize)]) -> Option<Vec<Piece>> {
    let mut pieces = Vec::new();
    let mut prev = 0;
    for &(start, end) in groups {
        push_literal(&mut pieces, &template[prev..start])?;
        pieces.push(Piece::Plus);
        prev = end;
    }
    push_literal(&mut pieces, &template[prev..])?;
    Some(pieces)
}

/// Matches `pieces` at `s[pos..]` as an ECMAScript backtracking matcher does:
/// each `(.+)` and `(.*)` first takes as many characters as it can, then gives
/// them back one at a time until the rest matches. With `to_end` the match
/// must end at the end of `s`. Returns the match's end and pushes the ranges
/// of its `(.+)`s onto `captures`.
fn match_at(
    pieces: &[Piece],
    s: &[char],
    pos: usize,
    to_end: bool,
    captures: &mut Vec<(usize, usize)>,
) -> Option<usize> {
    let Some((&piece, rest)) = pieces.split_first() else {
        return (!to_end || pos == s.len()).then_some(pos);
    };
    match piece {
        Piece::Char(c) => match s.get(pos) {
            Some(&d) if d == c => match_at(rest, s, pos + 1, to_end, captures),
            _ => None,
        },
        Piece::Any => match s.get(pos) {
            Some(&d) if !is_line_terminator(d) => match_at(rest, s, pos + 1, to_end, captures),
            _ => None,
        },
        Piece::Plus | Piece::Star => {
            let min = usize::from(piece == Piece::Plus);
            let max = s[pos..].iter().take_while(|&&c| !is_line_terminator(c)).count();
            for len in (min..=max).rev() {
                let mark = captures.len();
                if piece == Piece::Plus {
                    captures.push((pos, pos + len));
                }
                if let Some(end) = match_at(rest, s, pos + len, to_end, captures) {
                    return Some(end);
                }
                captures.truncate(mark);
            }
            None
        }
    }
}

/// `std::regex_match`: whether `pieces` match all of `s`.
fn full_match(pieces: &[Piece], s: &[char]) -> bool {
    match_at(pieces, s, 0, true, &mut Vec::new()).is_some()
}

/// `std::sregex_iterator`: the successive leftmost matches of `pieces` in
/// `s`, each search starting where the last match ended.
fn find_iter(pieces: &[Piece], s: &[char]) -> Vec<Match> {
    let mut matches = Vec::new();
    let mut pos = 0;
    while pos <= s.len() {
        let found = (pos..=s.len()).find_map(|start| {
            let mut captures = Vec::new();
            match_at(pieces, s, start, false, &mut captures).map(|end| Match { start, end, captures })
        });
        let Some(m) = found else {
            break;
        };
        pos = if m.end > m.start { m.end } else { m.end + 1 };
        matches.push(m);
    }
    matches
}

/// The char ranges of `template`'s `{domain}`, `{path}`, `{directory}`,
/// `{filename}` and `{extension}` tokens, in order (the matches of
/// `\{(domain|path|directory|filename|extension)\}`).
fn token_ranges(template: &[char]) -> Vec<(usize, usize)> {
    const TOKENS: [&str; 5] = ["{domain}", "{path}", "{directory}", "{filename}", "{extension}"];
    let mut ranges = Vec::new();
    let mut pos = 0;
    while pos < template.len() {
        let token = TOKENS.iter().find(|token| {
            let len = token.chars().count();
            template.get(pos..pos + len).is_some_and(|text| text.iter().copied().eq(token.chars()))
        });
        match token {
            Some(token) => {
                let end = pos + token.chars().count();
                ranges.push((pos, end));
                pos = end;
            }
            None => pos += 1,
        }
    }
    ranges
}

/// `createTokenMap(urlTemplate, url)`.
fn create_token_map(url_template: &str, url: &str) -> BTreeMap<String, String> {
    let parsed_url = URL::new(url);

    // e.g. tiles/satellite/tiles.json
    let path: Vec<char> = substr(url, parsed_url.path).chars().collect();

    // --- find tokens in the urlTemplate (such as {path}, ...)
    let template: Vec<char> = url_template.chars().collect();
    let tokens = token_ranges(&template);
    let mut token_list: Vec<(String, String)> =
        tokens.iter().map(|&(start, end)| (template[start + 1..end - 1].iter().collect(), String::new())).collect();

    // --- extract token values
    // /tiles{path}/tiles.json in tiles/satellite/tiles.json
    // e.g. path = /satellite
    // Each match of the template pattern gives the value of one token: its
    // first group.
    if let Some(pattern) = pattern_with_groups(&template, &tokens) {
        let mut idx = 0;
        for m in find_iter(&pattern, &path) {
            if let Some(&(start, end)) = m.captures.first() {
                // C++ indexes `tokenList[idx]` without a bounds check.
                if let Some(token) = token_list.get_mut(idx) {
                    token.1 = path[start..end].iter().collect();
                }
                idx += 1;
            }
        }
    }

    // `std::copy` into a `std::map`: the first of equal keys wins.
    let mut token_map = BTreeMap::new();
    for (key, value) in token_list {
        token_map.entry(key).or_insert(value);
    }
    token_map
}

/// `isNormalizedSourceURL(baseURL, urlTemplate, url)`: whether `url` matches
/// `^(?:` baseURL, the template with `\{.+\}` replaced by `(.+)`, `(.*))$`.
fn is_normalized_source_url(base_url: &str, url_template: &str, url: &str) -> bool {
    if base_url.is_empty() || url_template.is_empty() {
        return false;
    }

    let template: Vec<char> = url_template.chars().collect();
    let braces = [Piece::Char('{'), Piece::Plus, Piece::Char('}')];
    let groups: Vec<(usize, usize)> = find_iter(&braces, &template).iter().map(|m| (m.start, m.end)).collect();

    let mut pattern = Vec::new();
    let base_url: Vec<char> = base_url.chars().collect();
    if push_literal(&mut pattern, &base_url).is_none() {
        return false;
    }
    let Some(template_pattern) = pattern_with_groups(&template, &groups) else {
        return false;
    };
    pattern.extend(template_pattern);
    pattern.push(Piece::Star);

    full_match(&pattern, &url.chars().collect::<Vec<_>>())
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

/// `canonicalizeTileURL(tileServerOptions, str, type, tileSize)`. `raster` is
/// `type == SourceType::Raster || type == SourceType::RasterDEM` (mln_util
/// sits below the style types).
pub fn canonicalize_tile_url(tile_server_options: &TileServerOptions, s: &str, raster: bool, tile_size: u16) -> String {
    if !is_normalized_url(tile_server_options, s) {
        return s.to_owned();
    }

    let mut result = format!("{}://{}", tile_server_options.uri_scheme_alias(), tile_server_options.tile_domain_name());

    let url = URL::new(s);
    let path = Path::with_range(s, url.path.0, url.path.1);

    let version_prefix_len = tile_server_options.tile_version_prefix().as_ref().map_or(0, String::len);

    result += substr(s, (path.directory.0 + version_prefix_len, path.directory.1.wrapping_sub(version_prefix_len)));
    result += substr(s, path.filename);
    if raster {
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

/// `canonicalizeTileset(tileServerOptions, tileset, sourceURL, type, tileSize)`,
/// with `raster` as in [`canonicalize_tile_url`].
pub fn canonicalize_tileset(
    tile_server_options: &TileServerOptions,
    tileset: &mut Tileset,
    source_url: &str,
    raster: bool,
    tile_size: u16,
) {
    if is_canonical_url(tile_server_options, source_url)
        || is_normalized_source_url(tile_server_options.base_url(), tile_server_options.source_template(), source_url)
    {
        for url in tileset.tiles.iter_mut() {
            *url = canonicalize_tile_url(tile_server_options, url, raster, tile_size);
        }
    }
}

/// The shared body of `canonicalize{Source,Sprite,Glyph}URL`.
fn canonicalize(
    tile_server_options: &TileServerOptions,
    url: &str,
    template: &str,
    domain_name: &str,
    version_prefix: &Option<String>,
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
pub fn canonicalize_source_url(tile_server_options: &TileServerOptions, url: &str) -> String {
    canonicalize(
        tile_server_options,
        url,
        tile_server_options.source_template(),
        tile_server_options.source_domain_name(),
        tile_server_options.source_version_prefix(),
    )
}

/// `canonicalizeSpriteURL(tileServerOptions, url)`.
pub fn canonicalize_sprite_url(tile_server_options: &TileServerOptions, url: &str) -> String {
    canonicalize(
        tile_server_options,
        url,
        tile_server_options.sprites_template(),
        tile_server_options.sprites_domain_name(),
        tile_server_options.sprites_version_prefix(),
    )
}

/// `canonicalizeGlyphURL(tileServerOptions, url)`.
pub fn canonicalize_glyph_url(tile_server_options: &TileServerOptions, url: &str) -> String {
    // "https://api.maptiler.com/fonts/{fontstack}/{range}.pbf?key=uwvyvzaF2P7UWbyOEvjU"
    canonicalize(
        tile_server_options,
        url,
        tile_server_options.glyphs_template(),
        tile_server_options.glyphs_domain_name(),
        tile_server_options.glyphs_version_prefix(),
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

    #[test]
    fn maptiler_canonical_urls() {
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
            canonicalize_tile_url(&options, "https://api.maptiler.com/tiles/v3/{z}/{x}/{y}.pbf?key=abc", false, 512)
        );
        assert_eq!(3, nth_occurrence_from_end("api.maptiler.com", ".", 2));
    }

    // The expected values are what JavaScript's RegExp, an ECMAScript
    // engine, gives for the same patterns.

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn pattern(template: &str) -> Vec<Piece> {
        let template = chars(template);
        pattern_with_groups(&template, &token_ranges(&template)).expect("a supported template")
    }

    /// The `(.+)` captures of a whole-string match of `pattern` on `s`.
    fn full_captures(pattern: &[Piece], s: &str) -> Option<Vec<String>> {
        let s = chars(s);
        let mut captures = Vec::new();
        match_at(pattern, &s, 0, true, &mut captures)?;
        Some(captures.iter().map(|&(start, end)| s[start..end].iter().collect()).collect())
    }

    #[test]
    fn first_group_is_greedy_over_repeated_literals() {
        // /^(?:\/(.+)\/(.+))$/ on "/a/b/c"
        assert_eq!(
            Some(vec!["a/b".to_owned(), "c".to_owned()]),
            full_captures(&pattern("/{path}/{filename}"), "/a/b/c")
        );
        // /^(?:(.+)x(.+))$/ on "axbxc"
        let mut axb = vec![Piece::Plus, Piece::Char('x'), Piece::Plus];
        assert_eq!(Some(vec!["axb".to_owned(), "c".to_owned()]), full_captures(&axb, "axbxc"));
        axb.push(Piece::Char('x'));
        assert_eq!(None, full_captures(&axb, "axbxc"));
    }

    #[test]
    fn group_keeps_the_literal_that_follows_it() {
        // /\/tiles(.+)\/tiles.json/ in "/tiles/a/tiles/b/tiles.json"
        let token_map = create_token_map("/tiles{path}/tiles.json", "https://host/tiles/a/tiles/b/tiles.json");
        assert_eq!(Some(&"/a/tiles/b".to_owned()), token_map.get("path"));
    }

    #[test]
    fn dot_in_a_template_matches_any_character() {
        let base = "https://api.mapbox.com";
        assert!(is_normalized_source_url(base, "/{domain}.json", "https://apixmapbox.com/a.json"));
        assert!(is_normalized_source_url(base, "/{domain}.json", "https://api.mapbox.com/v4/a.b.jsonx?k=1"));
        assert!(!is_normalized_source_url(base, "/{domain}.json", "https://api.mapbox.com/.json"));
    }

    #[test]
    fn braces_become_one_group_from_the_first_to_the_last() {
        // "/font/{fontstack}/{start}-{end}.pbf".replace(/\{.+\}/g, "(.+)") is "/font/(.+).pbf".
        let base = "https://demotiles.maplibre.org";
        let template = "/font/{fontstack}/{start}-{end}.pbf";
        assert!(is_normalized_source_url(base, template, "https://demotiles.maplibre.org/font/a/b.pbf"));
        assert!(!is_normalized_source_url(base, template, "https://demotiles.maplibre.org/font/.pbf"));
    }

    #[test]
    fn groups_stop_at_line_terminators() {
        assert_eq!(None, full_captures(&pattern("a{path}b"), "a\nb"));
        // [..."{a}\n{b}".matchAll(/\{(.+)\}/g)]
        let braces = [Piece::Char('{'), Piece::Plus, Piece::Char('}')];
        let matches = find_iter(&braces, &chars("{a}\n{b}"));
        assert_eq!(vec![(0, 3), (4, 7)], matches.iter().map(|m| (m.start, m.end)).collect::<Vec<_>>());
        assert_eq!(vec![(1, 2), (5, 6)], matches.iter().map(|m| m.captures[0]).collect::<Vec<_>>());
    }

    #[test]
    fn other_regex_syntax_never_matches() {
        let base = "https://api.mapbox.com";
        assert!(!is_normalized_source_url(base, "/{domain}.json?x", "https://api.mapbox.com/a.json?x"));
        assert!(!is_normalized_source_url("https://api.mapbox.com(", "/{domain}", "https://api.mapbox.com(/a"));
        assert!(create_token_map("/{path}+", "https://host/a+").get("path").is_some_and(String::is_empty));
    }
}
