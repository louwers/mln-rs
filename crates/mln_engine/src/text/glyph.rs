use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU16;
use std::sync::Arc;

use mln_style::bitmap::AlphaImage;
use mln_util::font_stack::FontStack;
use mln_vec_map::VecMap;
use parking_lot::Mutex;

use crate::text::atlas::RectU16;
use crate::text::tagged_string::FontStackHash;

/// Where a glyph comes from: glyph PBFs, or a font file that shapes text,
/// with a type numbered at runtime ([`face_type`]).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, mln_wire::Wire)]
pub enum GlyphIDType {
    #[default]
    FontPBF,
    Face(NonZeroU16),
}

impl GlyphIDType {
    /// Text whose fonts the device chooses per character
    /// ([`FontFace::system`]): the glyph store has the renderer's
    /// [`FontChooser`](crate::text::local_fonts::FontChooser) choose a face for
    /// each piece, and its glyphs take those faces' types. No glyph has this
    /// type, and no face its number. A constant rather than a variant, so
    /// that a type stays a `u16`.
    pub const SYSTEM: GlyphIDType = GlyphIDType::Face(NonZeroU16::MAX);

    /// The type of the font face numbered `id`.
    #[must_use]
    pub const fn face(id: NonZeroU16) -> Self {
        GlyphIDType::Face(id)
    }

    /// The type's number as stored in a glyph ID's upper half: 0 for
    /// `FontPBF`.
    #[must_use]
    pub const fn id(self) -> u16 {
        match self {
            GlyphIDType::FontPBF => 0,
            GlyphIDType::Face(id) => id.get(),
        }
    }
}

/// `GLYPHS_PER_GLYPH_RANGE` etc. (`glyph_range.hpp`).
pub const GLYPHS_PER_GLYPH_RANGE: u32 = 256;
pub const GLYPH_RANGES_PER_FONT_STACK: u32 = 256;
pub const NON_IDEOGRAPH_GLYPH_RANGES_PER_FONT_STACK: u32 = 97;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct GlyphRange {
    pub first: u16,
    pub second: u16,
    pub type_: GlyphIDType,
}

impl GlyphRange {
    /// The range `first..=second` of `type_`. The bounds are truncated to
    /// `u16`, as in C++.
    #[must_use]
    pub fn new(first: u32, second: u32, type_: GlyphIDType) -> Self {
        GlyphRange { first: first as u16, second: second as u16, type_ }
    }
}

impl PartialOrd for GlyphRange {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GlyphRange {
    /// Orders by first, then second, then type.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.first, self.second, self.type_).cmp(&(other.first, other.second, other.type_))
    }
}

/// A glyph: `code` (a BMP character in glyph PBFs, or a font's glyph index)
/// and `type`; `hash` is the two read as one little-endian `char32_t`, which
/// is what C++ compares.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, mln_wire::Wire)]
pub struct GlyphID {
    pub code: u16,
    pub type_: GlyphIDType,
}

impl GlyphID {
    /// The glyph for `codepoint`, truncated to 16 bits, of type `FontPBF`.
    #[must_use]
    pub fn new(codepoint: u32) -> Self {
        GlyphID { code: codepoint as u16, type_: GlyphIDType::FontPBF }
    }

    /// The glyph PBF glyph of `c`. Glyph PBFs only cover the BMP, so a
    /// character beyond it has none.
    #[must_use]
    pub fn from_char(c: char) -> Option<Self> {
        pbf_code(c).map(GlyphID::from)
    }

    /// The character that a glyph PBF glyph stands for; `None` for a
    /// surrogate code.
    #[must_use]
    pub fn char(self) -> Option<char> {
        char::from_u32(u32::from(self.code))
    }

    #[must_use]
    pub fn with_type(index: u16, type_: GlyphIDType) -> Self {
        GlyphID { code: index, type_ }
    }

    /// The `hash` member of the union (x86-64 / arm64 are little endian).
    #[must_use]
    pub(crate) fn hash(self) -> u32 {
        (u32::from(self.type_.id()) << 16) | u32::from(self.code)
    }
}

impl PartialOrd for GlyphID {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for GlyphID {
    /// Compares `hash`.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.hash().cmp(&other.hash())
    }
}

impl From<u16> for GlyphID {
    fn from(code: u16) -> Self {
        GlyphID::new(u32::from(code))
    }
}

/// The code of `c`'s glyph in glyph PBFs, which only cover the BMP.
#[must_use]
pub(crate) fn pbf_code(c: char) -> Option<u16> {
    u16::try_from(u32::from(c)).ok()
}

pub type GlyphIDs = BTreeSet<GlyphID>;

/// The 256-glyph range that contains `glyph`. Only works for the BMP.
#[must_use]
pub fn get_glyph_range(glyph: GlyphID) -> GlyphRange {
    let start = (u32::from(glyph.code) / 256) * 256;
    let end = start + 255;
    let start = start.min(65280);
    let end = end.min(65535);
    GlyphRange::new(start, end, glyph.type_)
}

#[derive(Clone, Copy, PartialEq, Debug, Default, mln_wire::Wire)]
pub struct GlyphMetrics {
    pub width: u32,
    pub height: u32,
    pub left: f32,
    pub top: f32,
    pub advance: u32,
    /// True when the glyph bitmap is rendered at 2x texture resolution
    /// (e.g. local CJK glyphs).
    pub is_double_resolution: bool,
}

/// `Glyph`.
#[derive(Clone, PartialEq, Debug, Default, mln_wire::Wire)]
pub struct Glyph {
    pub id: GlyphID,
    /// A signed distance field of the glyph with a border (see `BORDER_SIZE`).
    pub bitmap: AlphaImage,
    pub metrics: GlyphMetrics,
}

impl Glyph {
    /// The SDF border around each glyph bitmap. We're using this value
    /// throughout the Mapbox GL ecosystem. If this is different, the glyphs
    /// also need to be reencoded.
    pub(crate) const BORDER_SIZE: u8 = 3;
}

pub type Glyphs = BTreeMap<GlyphID, Option<Arc<Glyph>>>;
pub type GlyphMap = VecMap<FontStackHash, Glyphs>;

/// A glyph or an inline image of a shaped label, placed relative to the
/// label's anchor, in pixels at a font size of `ONE_EM`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PositionedGlyph {
    pub glyph: GlyphID,
    pub x: f32,
    pub y: f32,
    /// Whether the glyph stands upright in a vertical line rather than
    /// turning with it.
    pub vertical: bool,
    pub scale: f32,
    /// Where the glyph or image is in its atlas.
    pub rect: RectU16,
    pub metrics: GlyphMetrics,
    /// The image an image section draws.
    pub image_id: Option<String>,
    /// The index of the glyph's section in its `TaggedString`.
    pub section_index: usize,
}

/// A line of a shaped label.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PositionedLine {
    pub glyphs: Vec<PositionedGlyph>,
    /// How far the line's content reaches below a line of `ONE_EM` glyphs:
    /// its largest section's growth beyond one em, or an inline image's.
    pub line_offset: f32,
}

/// A box around a label's anchor: its edges' offsets from the anchor, with y
/// down.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Bounds {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Bounds {
    #[must_use]
    pub fn width(&self) -> f32 {
        self.right - self.left
    }

    #[must_use]
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }
}

/// A label's text, broken into lines and placed glyph by glyph.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shaping {
    pub lines: Vec<PositionedLine>,
    /// The box the text takes, moved by the layout's offset.
    pub bounds: Bounds,
    /// Whether a glyph stands upright, as in vertical text.
    pub verticalizable: bool,
    /// Whether the text holds an inline image.
    pub icons_in_text: bool,
}

impl Shaping {
    /// The y of a label's first line before alignment moves it, which suits
    /// glyph PBFs' metrics. It belongs in the font's metadata.
    pub const Y_OFFSET: f32 = -17.0;

    /// Whether no line has a glyph.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|line| line.glyphs.is_empty())
    }
}

pub use mln_style::font_face::FontFaceRange;

/// The bytes of a font file that an app already holds, such as a
/// memory-mapped file, shared without a copy, and which of its faces to use.
/// A font face's file can be TrueType or OpenType, a collection of them, WOFF
/// or WOFF2.
#[derive(Clone)]
pub struct FontData {
    bytes: Arc<dyn AsRef<[u8]> + Send + Sync>,
    face_index: u32,
}

impl FontData {
    /// The file's first face.
    pub fn new(bytes: Arc<dyn AsRef<[u8]> + Send + Sync>) -> Self {
        FontData { bytes, face_index: 0 }
    }

    /// Face `index` of a collection, rather than its first.
    #[must_use]
    pub fn with_face_index(self, index: u32) -> Self {
        FontData { face_index: index, ..self }
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        (*self.bytes).as_ref()
    }

    /// Which face of a collection to use: 0 for a file of one face.
    #[must_use]
    pub fn face_index(&self) -> u32 {
        self.face_index
    }

    /// The same bytes, for readers that take any shared bytes.
    #[must_use]
    pub(crate) fn shared(&self) -> Arc<dyn AsRef<[u8]> + Send + Sync> {
        Arc::clone(&self.bytes)
    }
}

impl Default for FontData {
    fn default() -> Self {
        FontData::from(Vec::new())
    }
}

impl From<Vec<u8>> for FontData {
    fn from(bytes: Vec<u8>) -> Self {
        FontData::new(Arc::new(bytes))
    }
}

impl From<Arc<Vec<u8>>> for FontData {
    fn from(bytes: Arc<Vec<u8>>) -> Self {
        FontData::new(bytes)
    }
}

impl std::fmt::Debug for FontData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Not the bytes: they may load on first access.
        f.debug_struct("FontData").field("face_index", &self.face_index).finish_non_exhaustive()
    }
}

/// Equal when both share the same bytes and use the same face.
impl PartialEq for FontData {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.bytes, &other.bytes) && self.face_index == other.face_index
    }
}

/// A font file that draws a font's characters in `ranges`, from a style's
/// `font-faces` or an app's fallback faces. A face without a name is a
/// fallback, which every font stack uses after the faces that name one of
/// its fonts: in a style with glyph PBFs, for the text they can't shape; or
/// one of the device's fonts ([`FontFace::device`]), which has no ranges.
#[derive(Clone, Debug, PartialEq, mln_wire::Wire)]
pub struct FontFace {
    /// The type of the face's glyphs.
    pub type_: GlyphIDType,
    /// The `text-font` name it draws, or "" for a fallback.
    pub name: String,
    pub url: String,
    /// The code points it draws, first and last.
    pub ranges: Vec<FontFaceRange>,
    /// The font file itself, which the map then uses instead of fetching
    /// `url`. Left out of the faces that workers get, which don't read
    /// font files.
    #[wire(skip)]
    pub data: Option<FontData>,
    /// The OpenType weight a variable font loads at: regular (400) unless
    /// made at another. Left out of the faces that workers get, like `data`.
    #[wire(skip, default = "regular_weight")]
    pub weight: u16,
}

/// [`REGULAR_WEIGHT`](crate::text::weight::REGULAR_WEIGHT) as an OpenType weight.
fn regular_weight() -> u16 {
    crate::text::weight::REGULAR_WEIGHT as u16
}

impl FontFace {
    #[must_use]
    pub fn new(name: &str, url: &str, ranges: Vec<FontFaceRange>) -> Self {
        FontFace {
            type_: GlyphIDType::FontPBF,
            name: name.to_string(),
            url: url.to_string(),
            ranges,
            data: None,
            weight: regular_weight(),
        }
    }

    /// A fallback face, without a name, for the font file at `url`, which
    /// draws the code points in `ranges`, with its glyph type.
    #[must_use]
    pub fn fallback(url: &str, ranges: Vec<FontFaceRange>) -> Self {
        let mut face = FontFace::new("", url, ranges);
        face.type_ = face_type(&face.url, "", &face.ranges);
        face
    }

    /// The device's fonts, as a fallback face: in a style with glyph PBFs,
    /// for the text in `ranges` (the scripts the PBFs can't shape that the
    /// device has fonts for); in a style without, for all the text the
    /// style's own faces don't draw. For each character, the glyph store has
    /// the renderer's [`FontChooser`](crate::text::local_fonts::FontChooser) choose
    /// the face that draws it.
    #[must_use]
    pub fn system(ranges: Vec<FontFaceRange>) -> Self {
        FontFace { type_: GlyphIDType::SYSTEM, ..FontFace::new("", "", ranges) }
    }

    /// One of the device's fonts, which a
    /// [`FontChooser`](crate::text::local_fonts::FontChooser) chose, at the
    /// OpenType `weight` its label's font name asks for. It has no name and
    /// no ranges, holds its font file, and `url` only names it
    /// (`file:///path#index`, or `font://PostScriptName` for a CoreText
    /// font). The same url and weight get the same type for the life of the
    /// process.
    #[must_use]
    pub fn device(url: &str, weight: u16, data: FontData) -> Self {
        FontFace {
            type_: face_type(&format!("{url}@{weight}"), "", &[]),
            weight,
            ..FontFace::new("", url, Vec::new()).with_data(data)
        }
    }

    /// The face with its font file in memory, so that `url` only names it.
    #[must_use]
    pub fn with_data(mut self, data: FontData) -> Self {
        self.data = Some(data);
        self
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        !self.name.is_empty() && !self.url.is_empty() && !self.ranges.is_empty()
    }
}

/// A style's face, or an app's fallback face without a name, with its glyph
/// type.
impl From<&mln_style::font_face::FontFace> for FontFace {
    fn from(face: &mln_style::font_face::FontFace) -> Self {
        FontFace {
            type_: face_type(&face.url, &face.name, &face.ranges),
            ..FontFace::new(&face.name, &face.url, face.ranges.clone())
        }
    }
}

impl mln_style::font_face::FontCoverage for FontFace {
    fn covers(&self, c: char) -> bool {
        mln_style::font_face::ranges_cover(&self.ranges, c)
    }
}

pub type FontFaces = Vec<FontFace>;

/// The faces that a map draws a style's text with: the style's own
/// `font-faces`, then `fallback`, the app's. `None` when the style has no
/// `font-faces` and the app no fallback faces.
#[must_use]
pub fn style_font_faces(
    style: Option<&mln_style::font_face::FontFaces>,
    fallback: Option<&FontFaces>,
) -> Option<Arc<FontFaces>> {
    let fallback = fallback.filter(|faces| !faces.is_empty());
    if style.is_none() && fallback.is_none() {
        return None;
    }
    let style = style.into_iter().flatten().map(FontFace::from);
    Some(Arc::new(style.chain(fallback.into_iter().flatten().cloned()).collect()))
}

/// The texts that each font face of each font stack shapes.
pub type ShapeRequests = VecMap<FontStack, VecMap<GlyphIDType, BTreeSet<Vec<char>>>>;

#[derive(Clone, Debug, Default, PartialEq, mln_wire::Wire)]
pub struct GlyphDependencies {
    pub glyphs: VecMap<FontStack, GlyphIDs>,
    pub shapes: ShapeRequests,
}

/// Where a glyph is in an atlas, and its metrics. A glyph without a bitmap
/// (a space, say) has an empty `rect`.
#[derive(Clone, Copy, Debug, Default, PartialEq, mln_wire::Wire)]
pub struct GlyphPosition {
    pub rect: RectU16,
    pub metrics: GlyphMetrics,
}

/// The glyphs of a layout's font stacks: `None` for one that no font has, or
/// whose range failed to load.
pub type GlyphPositions = VecMap<FontStackHash, BTreeMap<GlyphID, Option<GlyphPosition>>>;

/// The font file, font and ranges a glyph type was handed out for.
type FaceKey = (String, String, Vec<FontFaceRange>);

/// The glyph types handed out, a few for each style: by what they were for,
/// and the last one.
struct FaceTypes {
    last: u16,
    types: Vec<(FaceKey, NonZeroU16)>,
}

static FACE_TYPES: Mutex<FaceTypes> = Mutex::new(FaceTypes { last: 0, types: Vec::new() });

/// The glyph type of the face of the font file at `url` that draws `font`'s
/// `ranges`: the same for the same three for the life of the process, and
/// a new one otherwise, which after `u16::MAX - 1` types repeats the first.
pub fn face_type(url: &str, font: &str, ranges: &[FontFaceRange]) -> GlyphIDType {
    let mut types = FACE_TYPES.lock();
    let found = types.types.iter().find(|((u, f, r), _)| u == url && f == font && r == ranges);
    if let Some(&(_, id)) = found {
        return GlyphIDType::Face(id);
    }
    // Neither 0 (`FontPBF`) nor `u16::MAX` (`SYSTEM`).
    types.last = if types.last >= u16::MAX - 1 { 1 } else { types.last + 1 };
    let id = NonZeroU16::new(types.last).unwrap_or(NonZeroU16::MIN);
    types.types.push(((url.to_owned(), font.to_owned(), ranges.to_vec()), id));
    GlyphIDType::Face(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_id_order_is_the_union_hash() {
        let a = GlyphID::with_type(0xFFFF, GlyphIDType::FontPBF);
        let b = GlyphID::with_type(0, GlyphIDType::face(NonZeroU16::MIN));
        assert!(a < b);
        assert_eq!(b.hash(), 0x10000);
        assert_eq!(GlyphID::new(0x1F600).code, 0xF600);
        assert_eq!(GlyphID::from_char('\u{FFFF}'), Some(GlyphID::new(0xFFFF)));
        assert_eq!(GlyphID::from_char('\u{1F600}'), None);
    }

    #[test]
    fn glyph_range() {
        let r = get_glyph_range(GlyphID::new(0x41));
        assert_eq!((r.first, r.second), (0, 255));
        let r = get_glyph_range(GlyphID::new(0xFFFF));
        assert_eq!((r.first, r.second), (65280, 65535));
    }

    // Layouts compare and hash glyph IDs by the million.
    #[test]
    fn a_glyph_id_is_two_u16s() {
        assert_eq!(size_of::<GlyphIDType>(), 2);
        assert_eq!(size_of::<GlyphID>(), 4);
    }

    #[test]
    fn face_types_are_stable_per_key() {
        let fs = "Noto Sans Regular";
        let a = face_type("local://a.ttf", fs, &[(0x1780, 0x17FF)]);
        let b = face_type("local://a.ttf", fs, &[(0x1780, 0x17FF)]);
        let c = face_type("local://b.ttf", fs, &[(0x1780, 0x17FF)]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, GlyphIDType::FontPBF);
    }

    #[test]
    fn style_faces_come_before_fallback_faces() {
        let style_face = mln_style::font_face::FontFace {
            name: "Noto Sans Regular".to_owned(),
            url: "local://a.ttf".to_owned(),
            ranges: vec![(0x1000, 0x109F)],
        };
        let fallback = vec![FontFace::system(vec![(0x0900, 0x097F)])];

        assert_eq!(style_font_faces(None, None), None);
        assert_eq!(style_font_faces(None, Some(&Vec::new())), None);
        assert_eq!(style_font_faces(Some(&Vec::new()), None), Some(Arc::new(Vec::new())));

        let faces = style_font_faces(Some(&vec![style_face.clone()]), Some(&fallback)).unwrap();
        assert_eq!(faces.len(), 2);
        assert_eq!(faces[0].name, style_face.name);
        assert_eq!(faces[0].type_, face_type(&style_face.url, &style_face.name, &style_face.ranges));
        assert_eq!(faces[1], fallback[0]);
    }
}
