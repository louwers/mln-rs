// Ports include/mln/storage/resource.hpp and src/mln/storage/resource.cpp.

use mln_util::chrono::{Duration, Timestamp};
use mln_util::constants::{EARTH_RADIUS_M, M2PI};
use mln_util::font_stack::{FontStack, font_stack_to_string};
use mln_util::string::{to_string_double, to_string_i32};
use mln_util::tileset::Scheme;
use mln_util::token::replace_tokens;
use mln_util::url::{URL, percent_encode, substr};

use crate::response::Data;

/// `Resource::Kind` (the values are stored in the offline database).
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

/// `Resource::Priority`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Priority {
    #[default]
    Regular,
    Low,
}

/// `Resource::Usage`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Usage {
    #[default]
    Online,
    Offline,
}

/// `Resource::StoragePolicy`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StoragePolicy {
    #[default]
    Permanent,
    Volatile,
}

/// `Resource::TileData`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileData {
    pub url_template: String,
    pub pixel_ratio: u8,
    pub x: i32,
    pub y: i32,
    pub z: i8,
}

/// `Resource::LoadingMethod` (a bitmask).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LoadingMethod(pub u8);

#[allow(non_upper_case_globals)]
impl LoadingMethod {
    pub const None: LoadingMethod = LoadingMethod(0b00);
    pub const Cache: LoadingMethod = LoadingMethod(0b01);
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
    pub url: String,

    /// Includes auxiliary data if this is a tile request.
    pub tile_data: Option<TileData>,

    pub data_range: Option<(u64, u64)>,
    pub prior_modified: Option<Timestamp>,
    pub prior_expires: Option<Timestamp>,
    pub prior_etag: Option<String>,
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

impl Resource {
    /// `Resource(kind, url, tileData = nullopt, loadingMethod = All)`.
    pub fn new(kind: Kind, url: impl Into<String>) -> Self {
        Self::with_tile_data(kind, url, None, LoadingMethod::All)
    }

    pub fn with_tile_data(
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
            url: url.into(),
            tile_data,
            data_range: None,
            prior_modified: None,
            prior_expires: None,
            prior_etag: None,
            prior_data: None,
            minimum_update_interval: Duration::zero(),
            storage_policy: StoragePolicy::Permanent,
        }
    }

    pub fn set_priority(&mut self, p: Priority) {
        self.priority = p;
    }

    pub fn set_usage(&mut self, u: Usage) {
        self.usage = u;
    }

    /// `hasLoadingMethod`.
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
        let url = URL::new(base);
        Resource::new(
            Kind::SpriteImage,
            substr(base, 0, url.path.0.wrapping_add(url.path.1)).to_owned()
                + if pixel_ratio > 1.0 { "@2x" } else { "" }
                + ".png"
                + substr(base, url.query.0, url.query.1),
        )
    }

    #[must_use]
    pub fn sprite_json(base: &str, pixel_ratio: f32) -> Resource {
        let url = URL::new(base);
        Resource::new(
            Kind::SpriteJSON,
            substr(base, 0, url.path.0.wrapping_add(url.path.1)).to_owned()
                + if pixel_ratio > 1.0 { "@2x" } else { "" }
                + ".json"
                + substr(base, url.query.0, url.query.1),
        )
    }

    #[must_use]
    pub fn glyphs(url_template: &str, font_stack: &FontStack, glyph_range: (u16, u16)) -> Resource {
        Resource::new(
            Kind::Glyphs,
            replace_tokens(url_template, |token: &str| -> Option<String> {
                if token == "fontstack" {
                    Some(percent_encode(&font_stack_to_string(font_stack)))
                } else if token == "range" {
                    Some(glyph_range.0.to_string() + "-" + &glyph_range.1.to_string())
                } else {
                    None
                }
            }),
        )
    }

    #[must_use]
    pub fn font_face(url: &str) -> Resource {
        Resource::new(Kind::Glyphs, url)
    }

    /// `Resource::tile(urlTemplate, pixelRatio, x, y, z, scheme, loadingMethod = All)`.
    #[must_use]
    pub fn tile(
        url_template: &str,
        pixel_ratio: f32,
        x: i32,
        y: i32,
        z: i8,
        scheme: Scheme,
        loading_method: LoadingMethod,
    ) -> Resource {
        let supports_ratio = url_template.contains("{ratio}");
        let mut y = y;
        if scheme == Scheme::TMS {
            y = (1i32 << z).wrapping_sub(y).wrapping_sub(1);
        }
        let hex = b"0123456789abcdef";
        let url = replace_tokens(url_template, |token: &str| -> Option<String> {
            match token {
                "z" => Some(to_string_i32(i32::from(z))),
                "x" => Some(to_string_i32(x)),
                "y" => Some(to_string_i32(y)),
                "quadkey" => Some(get_quad_key(x, y, z)),
                // The spec defines `{bbox}` as `{bbox-epsg-3857}` under every
                // projection mln-rs has: mercator, globe and vertical-perspective.
                "bbox" | "bbox-epsg-3857" => Some(get_tile_bbox(x, y, z)),
                // C++ indexes with `x % 16` (UB for negative x).
                "prefix" => Some(
                    [hex[x.rem_euclid(16) as usize] as char, hex[y.rem_euclid(16) as usize] as char].iter().collect(),
                ),
                "ratio" => Some(if pixel_ratio > 1.0 { "@2x".to_owned() } else { String::new() }),
                _ => None,
            }
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
            }),
            loading_method,
        )
    }
}

#[cfg(test)]
mod tests {
    // Ports test/storage/resource.test.cpp.
    use super::*;

    fn tile(url: &str, ratio: f32, x: i32, y: i32, z: i8, scheme: Scheme) -> Resource {
        Resource::tile(url, ratio, x, y, z, scheme, LoadingMethod::All)
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
    }
}
