use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use mln_style::string::to_string_double;
use mln_style::tileset::Scheme;

use crate::geo::constants::{EARTH_RADIUS_M, M2PI};
use crate::storage::chrono::Timestamp;
use crate::storage::response::{Caching, Data, Response};
use crate::storage::token::replace_tokens;
use crate::text::font_stack::{FontStack, font_stack_to_string};

/// Token that may appear in a tile URL template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TileTemplateToken {
    Z,
    X,
    Y,
    Quadkey,
    Bbox,
    BboxEpsg3857,
    Prefix,
    Ratio,
}

impl FromStr for TileTemplateToken {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "z" => Ok(Self::Z),
            "x" => Ok(Self::X),
            "y" => Ok(Self::Y),
            "quadkey" => Ok(Self::Quadkey),
            "bbox" => Ok(Self::Bbox),
            "bbox-epsg-3857" => Ok(Self::BboxEpsg3857),
            "prefix" => Ok(Self::Prefix),
            "ratio" => Ok(Self::Ratio),
            _ => Err(()),
        }
    }
}

/// The kind of a resource. The values are stored in the offline database.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Kind {
    Unknown = 0,
    Style,
    Source,
    Tile,
    Glyphs,
    SpriteImage,
    SpriteJSON,
    Image,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Priority {
    #[default]
    Regular,
    Low,
}

/// Where a request stands among the waiting requests of its [`Priority`]:
/// a file source that makes requests wait starts the one with the lowest
/// value first, and those with the same value in the order they came.
///
/// A clone shares the value, so whoever made a request can change where it
/// stands while it waits, through the [`Resource`] it kept. A request that
/// has started isn't affected.
#[derive(Clone, Debug, Default)]
pub struct RequestOrder(Arc<AtomicU32>);

impl RequestOrder {
    #[must_use]
    pub fn get(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }

    pub(crate) fn set(&self, order: u32) {
        self.0.store(order, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Usage {
    #[default]
    Online,
    Offline,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StoragePolicy {
    #[default]
    Permanent,
    Volatile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileData {
    pub url_template: String,
    pub(crate) pixel_ratio: u8,
    pub x: i32,
    pub y: i32,
    pub z: i8,
    pub format: TileFormat,
}

/// What a tile's data is, which decides the media types that an HTTP
/// request for it accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileFormat {
    /// A Mapbox Vector Tile.
    Mvt,
    /// A MapLibre Tile.
    Mlt,
    /// An image: a raster or raster-dem tile.
    Image,
}

/// A bitmask of the ways a resource may be loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LoadingMethod(pub u8);

#[expect(non_upper_case_globals, reason = "the bit flags keep MapLibre Native's names")]
impl LoadingMethod {
    #[cfg(test)]
    pub(crate) const None: LoadingMethod = LoadingMethod(0b00);
    pub(crate) const Cache: LoadingMethod = LoadingMethod(0b01);
    pub const Network: LoadingMethod = LoadingMethod(0b10);

    pub const CacheOnly: LoadingMethod = Self::Cache;
    pub const NetworkOnly: LoadingMethod = Self::Network;
    pub const All: LoadingMethod = LoadingMethod(0b01 | 0b10);
}

/// `Resource`.
#[derive(Clone, Debug)]
pub struct Resource {
    pub kind: Kind,
    pub loading_method: LoadingMethod,
    pub usage: Usage,
    pub priority: Priority,
    /// Zero, ahead of every request that sets it, unless the requester says
    /// otherwise.
    pub order: RequestOrder,
    pub url: String,

    /// Includes auxiliary data if this is a tile request.
    pub tile_data: Option<TileData>,

    pub data_range: Option<(u64, u64)>,
    /// The copy of the resource the requester has or the cache holds, which
    /// a request revalidates: its validators and caching rules, and its data
    /// if the requester waits for it (see [`Resource::deliver`]).
    pub prior_modified: Option<Timestamp>,
    pub prior_etag: Option<String>,
    pub prior_caching: Caching,
    pub prior_data: Option<Data>,
    pub minimum_update_interval: Duration,
    pub storage_policy: StoragePolicy,
}

fn get_quad_key(x: i32, y: i32, z: i8) -> String {
    let mut quad_key = String::with_capacity(z.max(0) as usize);
    let mut i = z;
    while i > 0 {
        let mask: i32 = 1 << (i - 1);
        quad_key.push((b'0' + u8::from(x & mask != 0) + (if y & mask != 0 { 2 } else { 0 })) as char);
        i -= 1;
    }
    quad_key
}

fn get_merc_coord(x: i32, y: i32, z: i8) -> (f64, f64) {
    let resolution = (M2PI * EARTH_RADIUS_M / 256.0) / 2f64.powi(i32::from(z));
    (f64::from(x) * resolution - M2PI * EARTH_RADIUS_M / 2.0, f64::from(y) * resolution - M2PI * EARTH_RADIUS_M / 2.0)
}

fn get_tile_bbox(x: i32, y: i32, z: i8) -> String {
    // Alter the y for the Google/OSM tile scheme.
    let y = (2f64.powi(i32::from(z)) as i32).wrapping_sub(y).wrapping_sub(1);

    let min = get_merc_coord(x.wrapping_mul(256), y.wrapping_mul(256), z);
    let max = get_merc_coord(x.wrapping_add(1).wrapping_mul(256), y.wrapping_add(1).wrapping_mul(256), z);

    to_string_double(min.0)
        + ","
        + &to_string_double(min.1)
        + ","
        + &to_string_double(max.0)
        + ","
        + &to_string_double(max.1)
}

fn sprite_url(base: &str, pixel_ratio: f32, extension: &str) -> String {
    let fragment = base.find('#').unwrap_or(base.len());
    let query = base.find('?').filter(|&query| query < fragment);
    let path_end = query.unwrap_or(fragment);
    let suffix = if pixel_ratio > 1.0 { "@2x" } else { "" };
    format!("{}{suffix}{extension}{}", &base[..path_end], query.map_or("", |query| &base[query..fragment]))
}

fn encode_font_stack(font_stack: &FontStack) -> String {
    let hex = b"0123456789abcdef";
    let mut encoded = String::new();
    for byte in font_stack_to_string(font_stack).bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex[(byte >> 4) as usize] as char);
            encoded.push(hex[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

impl Resource {
    pub(crate) fn new(kind: Kind, url: impl Into<String>) -> Self {
        Self::with_tile_data(kind, url, None, LoadingMethod::All)
    }

    pub(crate) fn with_tile_data(
        kind: Kind,
        url: impl Into<String>,
        tile_data: Option<TileData>,
        loading_method: LoadingMethod,
    ) -> Self {
        Resource {
            kind,
            loading_method,
            usage: Usage::Online,
            priority: Priority::Regular,
            order: RequestOrder::default(),
            url: url.into(),
            tile_data,
            data_range: None,
            prior_modified: None,
            prior_etag: None,
            prior_caching: Caching::default(),
            prior_data: None,
            minimum_update_interval: Duration::ZERO,
            storage_policy: StoragePolicy::Permanent,
        }
    }

    /// Takes the validators and caching rules of `response`, a copy of the
    /// resource, as the prior copy's. The requester has that copy, so no data
    /// waits for it.
    pub fn set_prior(&mut self, response: &Response) {
        self.prior_data = None;
        self.prior_modified = response.modified;
        self.prior_etag.clone_from(&response.etag);
        self.prior_caching = response.caching;
    }

    /// What a revalidation's answer `response` delivers: a 304 keeps the
    /// prior copy's validators it lacks, and to a requester that waits for
    /// that copy in `prior_data`, delivers it. Any other answer as it is.
    #[must_use]
    pub fn deliver(&self, mut response: Response) -> Response {
        if !response.not_modified {
            return response;
        }
        response.modified = response.modified.or(self.prior_modified);
        if response.etag.is_none() {
            response.etag.clone_from(&self.prior_etag);
        }
        if let Some(data) = &self.prior_data {
            response.data = Some(Arc::clone(data));
            response.not_modified = false;
            response.prior = true;
        }
        response
    }

    #[cfg(any(test, feature = "sqlite"))]
    pub(crate) fn set_priority(&mut self, p: Priority) {
        self.priority = p;
    }

    #[cfg(feature = "sqlite")]
    pub(crate) fn set_usage(&mut self, u: Usage) {
        self.usage = u;
    }

    #[must_use]
    pub fn has_loading_method(&self, method: LoadingMethod) -> bool {
        (self.loading_method.0 & method.0) != 0
    }

    #[must_use]
    pub fn style(url: &str) -> Resource {
        Resource::new(Kind::Style, url)
    }

    #[must_use]
    pub fn source(url: &str) -> Resource {
        Resource::new(Kind::Source, url)
    }

    #[must_use]
    pub fn image(url: &str) -> Resource {
        Resource::new(Kind::Image, url)
    }

    #[must_use]
    pub fn sprite_image(base: &str, pixel_ratio: f32) -> Resource {
        Resource::new(Kind::SpriteImage, sprite_url(base, pixel_ratio, ".png"))
    }

    #[must_use]
    pub fn sprite_json(base: &str, pixel_ratio: f32) -> Resource {
        Resource::new(Kind::SpriteJSON, sprite_url(base, pixel_ratio, ".json"))
    }

    #[must_use]
    pub fn glyphs(url_template: &str, font_stack: &FontStack, glyph_range: (u16, u16)) -> Resource {
        Resource::new(
            Kind::Glyphs,
            replace_tokens(url_template, |token: &str| -> Option<String> {
                if token == "fontstack" {
                    Some(encode_font_stack(font_stack))
                } else if token == "range" {
                    Some(glyph_range.0.to_string() + "-" + &glyph_range.1.to_string())
                } else {
                    None
                }
            }),
        )
    }

    #[must_use]
    pub(crate) fn font_face(url: &str) -> Resource {
        Resource::new(Kind::Glyphs, url)
    }

    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "the tileset's template, the tile and its tileset's scheme and format"
    )]
    pub fn tile(
        url_template: &str,
        pixel_ratio: f32,
        x: i32,
        y: i32,
        z: i8,
        scheme: Scheme,
        format: TileFormat,
        loading_method: LoadingMethod,
    ) -> Resource {
        let supports_ratio = url_template.contains("{ratio}");
        let mut y = y;
        if scheme == Scheme::TMS {
            y = (1i32 << z).wrapping_sub(y).wrapping_sub(1);
        }
        let hex = b"0123456789abcdef";
        let url = replace_tokens(url_template, |token: &str| -> Option<String> {
            token.parse::<TileTemplateToken>().ok().map(|token| {
                match token {
                    TileTemplateToken::Z => z.to_string(),
                    TileTemplateToken::X => x.to_string(),
                    TileTemplateToken::Y => y.to_string(),
                    TileTemplateToken::Quadkey => get_quad_key(x, y, z),
                    // The spec defines `{bbox}` as `{bbox-epsg-3857}` under every
                    // projection mln-rs has: mercator, globe and vertical-perspective.
                    TileTemplateToken::Bbox | TileTemplateToken::BboxEpsg3857 => get_tile_bbox(x, y, z),
                    TileTemplateToken::Prefix => {
                        [hex[x.rem_euclid(16) as usize] as char, hex[y.rem_euclid(16) as usize] as char]
                            .iter()
                            .collect()
                    }
                    TileTemplateToken::Ratio => {
                        if pixel_ratio > 1.0 {
                            "@2x".to_owned()
                        } else {
                            String::new()
                        }
                    }
                }
            })
        });
        Resource::with_tile_data(
            Kind::Tile,
            url,
            Some(TileData {
                url_template: url_template.to_owned(),
                pixel_ratio: if supports_ratio && pixel_ratio > 1.0 { 2 } else { 1 },
                x,
                y,
                z,
                format,
            }),
            loading_method,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod deliver {
        use super::super::*;

        fn at(secs: i64) -> Timestamp {
            Timestamp::from_secs(1000 + secs)
        }

        fn waiting(caching: Caching) -> Resource {
            let mut resource = Resource::style("https://example.com/style.json");
            resource.prior_data = Some(Arc::new(b"cached".to_vec()));
            resource.prior_etag = Some("a".to_owned());
            resource.prior_caching = caching;
            resource
        }

        #[test]
        fn a_304_delivers_the_copy_with_its_caching_rules() {
            let resource = waiting(Caching { expires: Some(at(-10)), ..Caching::default() });
            let renewed = Response {
                not_modified: true,
                caching: Caching { expires: Some(at(100)), ..Caching::default() },
                ..Response::default()
            };
            let delivered = resource.deliver(renewed);
            assert!(!delivered.not_modified);
            assert_eq!(Some(b"cached".as_slice()), delivered.data.as_deref().map(Vec::as_slice));
            assert_eq!((Some(at(100)), Some("a")), (delivered.caching.expires, delivered.etag.as_deref()));
        }
    }

    fn tile(url: &str, ratio: f32, x: i32, y: i32, z: i8, scheme: Scheme) -> Resource {
        Resource::tile(url, ratio, x, y, z, scheme, TileFormat::Mvt, LoadingMethod::All)
    }

    fn check_tile(r: &Resource, url: &str, template: &str, pixel_ratio: u8, x: i32, y: i32, z: i8) {
        assert_eq!(Kind::Tile, r.kind);
        assert_eq!(url, r.url);
        let tile_data = r.tile_data.as_ref().unwrap();
        assert_eq!(template, tile_data.url_template);
        assert_eq!(pixel_ratio, tile_data.pixel_ratio);
        assert_eq!(x, tile_data.x);
        assert_eq!(y, tile_data.y);
        assert_eq!(z, tile_data.z);
    }

    // gtest: Resource.Style
    #[test]
    fn style() {
        let resource = Resource::style("http://example.com");
        assert_eq!(Kind::Style, resource.kind);
        assert_eq!("http://example.com", resource.url);
    }

    // gtest: Resource.Source
    #[test]
    fn source() {
        let resource = Resource::source("http://example.com");
        assert_eq!(Kind::Source, resource.kind);
        assert_eq!("http://example.com", resource.url);
    }

    // gtest: Resource.Tile
    #[test]
    fn tile_test() {
        let t = "http://example.com/{z}/{x}/{y}{ratio}.png";
        check_tile(&tile(t, 2.0, 1, 2, 3, Scheme::XYZ), "http://example.com/3/1/2@2x.png", t, 2, 1, 2, 3);

        let t = "http://example.com/{prefix}/{z}/{x}/{y}.mvt";
        check_tile(&tile(t, 2.0, 1, 2, 3, Scheme::XYZ), "http://example.com/12/3/1/2.mvt", t, 1, 1, 2, 3);

        let t = "http://example.com/{quadkey}.png";
        check_tile(&tile(t, 2.0, 0, 0, 1, Scheme::XYZ), "http://example.com/0.png", t, 1, 0, 0, 1);
        check_tile(&tile(t, 2.0, 0, 0, 2, Scheme::XYZ), "http://example.com/00.png", t, 1, 0, 0, 2);
        check_tile(&tile(t, 2.0, 1, 1, 2, Scheme::XYZ), "http://example.com/03.png", t, 1, 1, 1, 2);
        check_tile(
            &tile(t, 2.0, 22914, 52870, 17, Scheme::XYZ),
            "http://example.com/02301322130000230.png",
            t,
            1,
            22914,
            52870,
            17,
        );
        // Test case confirmed by quadkeytools package
        check_tile(&tile(t, 2.0, 29, 3, 6, Scheme::XYZ), "http://example.com/011123.png", t, 1, 29, 3, 6);

        let t = "http://example.com/?bbox={bbox-epsg-3857}";
        check_tile(
            &tile(t, 2.0, 0, 0, 1, Scheme::XYZ),
            "http://example.com/?bbox=-20037508.342789244,0,0,20037508.342789244",
            t,
            1,
            0,
            0,
            1,
        );

        let t = "http://example.com/{z}/{x}/{y}{ratio}.png";
        check_tile(&tile(t, 1.0, 1, 2, 3, Scheme::TMS), "http://example.com/3/1/5.png", t, 1, 1, 5, 3);
    }

    #[test]
    fn tile_bbox_matches_epsg_3857() {
        let t = "http://example.com/?bbox={bbox}";
        check_tile(
            &tile(t, 1.0, 0, 0, 1, Scheme::XYZ),
            "http://example.com/?bbox=-20037508.342789244,0,0,20037508.342789244",
            t,
            1,
            0,
            0,
            1,
        );
    }

    // gtest: Resource.Glyphs
    #[test]
    fn glyphs() {
        let resource = Resource::glyphs("http://example.com/{fontstack}/{range}", &vec!["stack".to_owned()], (0, 255));
        assert_eq!(Kind::Glyphs, resource.kind);
        assert_eq!("http://example.com/stack/0-255", resource.url);

        let resource = Resource::glyphs(
            "http://example.com/{fontstack}/{range}",
            &vec!["Open Sans".to_owned(), "cø".to_owned()],
            (0, 255),
        );
        assert_eq!("http://example.com/Open%20Sans%2cc%c3%b8/0-255", resource.url);
    }

    // gtest: Resource.SpriteImage
    #[test]
    fn sprite_image() {
        let resource = Resource::sprite_image("http://example.com/sprite", 2.0);
        assert_eq!(Kind::SpriteImage, resource.kind);
        assert_eq!("http://example.com/sprite@2x.png", resource.url);

        let param_resource = Resource::sprite_image("http://example.com/sprite?query=true", 2.0);
        assert_eq!(Kind::SpriteImage, param_resource.kind);
        assert_eq!("http://example.com/sprite@2x.png?query=true", param_resource.url);

        let fragment_resource = Resource::sprite_image("http://example.com/sprite?query=true#fragment", 2.0);
        assert_eq!("http://example.com/sprite@2x.png?query=true", fragment_resource.url);
    }

    // gtest: Resource.Image
    #[test]
    fn image() {
        let resource = Resource::image("http://example.com/sprite.jpg");
        assert_eq!(Kind::Image, resource.kind);
        assert_eq!("http://example.com/sprite.jpg", resource.url);
    }

    // gtest: Resource.SpriteJSON
    #[test]
    fn sprite_json() {
        let resource = Resource::sprite_json("http://example.com/sprite", 2.0);
        assert_eq!(Kind::SpriteJSON, resource.kind);
        assert_eq!("http://example.com/sprite@2x.json", resource.url);

        let param_resource = Resource::sprite_json("http://example.com/sprite?query=true", 2.0);
        assert_eq!(Kind::SpriteJSON, param_resource.kind);
        assert_eq!("http://example.com/sprite@2x.json?query=true", param_resource.url);

        let fragment_resource = Resource::sprite_json("http://example.com/sprite#fragment?ignored=true", 2.0);
        assert_eq!("http://example.com/sprite@2x.json", fragment_resource.url);
    }
}
