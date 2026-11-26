//! Where a system font's bytes come from, whether the map can draw a script
//! with it, and its bytes, mapped when the font is first chosen.

#![expect(unsafe_code, reason = "memory-mapping a font file is unsafe")]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use memmap2::Mmap;
use mln_style::hash::HashMap;
use mln_text::glyph::FontData;
use skrifa::raw::tables::cmap::Cmap;
use skrifa::raw::tables::fvar::Fvar;
use skrifa::raw::tables::name::Name;
use skrifa::raw::tables::os2::Os2;
use skrifa::raw::types::NameId;
use skrifa::raw::{FontData as RawData, FontRead};

use crate::log_target;
use crate::sfnt::{self, Table};

/// A font that the platform found.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    /// Face `index` of the font file at `path`.
    #[cfg_attr(target_vendor = "apple", expect(dead_code, reason = "CoreText's fonts come as tables"))]
    File { path: PathBuf, index: u32 },
    /// A font that CoreText holds.
    #[cfg(target_vendor = "apple")]
    CoreText(crate::apple::Font),
}

impl Source {
    /// A url that names the font, unique to it.
    pub(crate) fn url(&self) -> String {
        match self {
            Source::File { path, index: 0 } => format!("file://{}", path.display()),
            Source::File { path, index } => format!("file://{}#{index}", path.display()),
            #[cfg(target_vendor = "apple")]
            Source::CoreText(font) => format!("font://{}", font.name()),
        }
    }

    /// The font's tables `tags`, those it has.
    fn tables(&self, tags: &[[u8; 4]]) -> Option<Vec<Table>> {
        match self {
            Source::File { path, index } => sfnt::read_tables(path, *index, tags).ok(),
            #[cfg(target_vendor = "apple")]
            Source::CoreText(font) => Some(font.tables(tags)),
        }
    }

    /// The weight the map draws the font at when asked for `weight`, if it
    /// has a glyph for `probe`.
    pub(crate) fn drawn_weight(&self, probe: char, weight: u16) -> Option<u16> {
        drawn_weight(&self.tables(&[*b"cmap", *b"OS/2", *b"fvar"])?, probe, weight)
    }

    /// The font's family name, from its `name` table.
    #[cfg_attr(not(target_os = "android"), expect(dead_code, reason = "the other matchers name the family"))]
    pub(crate) fn family_name(&self) -> Option<String> {
        family_name(&self.tables(&[*b"name"])?)
    }
}

/// The family name in `tables`' `name` table: the typographic family, where
/// the font has one, else the family.
fn family_name(tables: &[Table]) -> Option<String> {
    let data = tables.iter().find(|(tag, _)| tag == b"name").map(|(_, data)| RawData::new(data))?;
    let name = Name::read(data).ok()?;
    let find = |id: NameId| {
        name.name_record()
            .iter()
            .filter(|record| record.name_id() == id)
            .find_map(|record| record.string(name.string_data()).ok())
            .map(|string| string.to_string())
    };
    find(NameId::TYPOGRAPHIC_FAMILY_NAME).or_else(|| find(NameId::FAMILY_NAME))
}

/// The font files of the chosen fonts, each shared by the faces that use it.
#[derive(Default)]
pub(crate) struct Files {
    /// Each file mapped, or `None` where it couldn't be.
    files: HashMap<PathBuf, Option<Arc<Mmap>>>,
    /// The font file made of each CoreText font's tables.
    #[cfg(target_vendor = "apple")]
    fonts: HashMap<crate::apple::Font, Arc<Vec<u8>>>,
}

impl Files {
    /// The bytes of `source`'s font: its file, memory-mapped, or a file of a
    /// CoreText font's tables. `None` for a file that can't be mapped, which
    /// logs a warning once.
    pub(crate) fn data(&mut self, source: &Source) -> Option<FontData> {
        match source {
            Source::File { path, index } => {
                let file = self.files.entry(path.clone()).or_insert_with(|| map(path).map(Arc::new)).clone()?;
                Some(FontData::new(file).with_face_index(*index))
            }
            #[cfg(target_vendor = "apple")]
            Source::CoreText(font) => {
                let file = self.fonts.entry(font.clone()).or_insert_with(|| Arc::new(font.file()));
                Some(FontData::from(Arc::clone(file)))
            }
        }
    }
}

/// The weight the map draws a font with `tables` at when asked for
/// `weight`, if it has a glyph for `probe`.
///
/// A variable font draws at `weight` within the range of its `wght` axis, a
/// static font at its own.
pub(crate) fn drawn_weight(tables: &[Table], probe: char, weight: u16) -> Option<u16> {
    let table = |tag: &[u8; 4]| tables.iter().find(|(t, _)| t == tag).map(|(_, data)| RawData::new(data));
    let cmap = Cmap::read(table(b"cmap")?).ok()?;
    cmap.map_codepoint(probe)?;
    let wght = table(b"fvar").and_then(|data| Fvar::read(data).ok()).and_then(|fvar| {
        let axes = fvar.axes().ok()?;
        let axis = axes.iter().find(|axis| axis.axis_tag() == skrifa::Tag::new(b"wght"))?;
        let range = axis.min_value().to_f64() as f32..=axis.max_value().to_f64() as f32;
        Some(f32::from(weight).clamp(*range.start(), *range.end()).round() as u16)
    });
    Some(
        wght.or_else(|| table(b"OS/2").and_then(|data| Os2::read(data).ok()).map(|os2| os2.us_weight_class()))
            .unwrap_or(400),
    )
}

/// The file at `path`, memory-mapped. A map reads only the pages it draws
/// from: a CJK font is tens of megabytes, and a map's labels use a few
/// thousand of its glyphs.
fn map(path: &Path) -> Option<Mmap> {
    let file = File::open(path)
        .inspect_err(|error| log::warn!(target: log_target::GLYPH, "Couldn't open {}: {error}", path.display()))
        .ok()?;
    // SAFETY: the mapping is read-only, and system font files aren't changed
    // in place: an update replaces a font by renaming a new file over it,
    // which leaves the mapped file as it was.
    unsafe { Mmap::map(&file) }
        .inspect_err(|error| log::warn!(target: log_target::GLYPH, "Couldn't map {}: {error}", path.display()))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::{drawn_weight, family_name};
    use crate::sfnt::Table;

    fn os2(weight: u16) -> Table {
        let mut table = vec![0; 78];
        table[4..6].copy_from_slice(&weight.to_be_bytes());
        (*b"OS/2", table)
    }

    /// An `fvar` with a `wght` axis from `min` to `max`.
    fn fvar(min: u16, max: u16) -> Table {
        let mut table = Vec::new();
        // version 1.0, axesArrayOffset 16, reserved, axisCount 1, axisSize
        // 20, instanceCount 0, instanceSize 8.
        for word in [1u16, 0, 16, 2, 1, 20, 0, 8] {
            table.extend_from_slice(&word.to_be_bytes());
        }
        table.extend_from_slice(b"wght");
        for value in [min, min, max] {
            table.extend_from_slice(&(u32::from(value) << 16).to_be_bytes());
        }
        table.extend_from_slice(&[0; 4]);
        (*b"fvar", table)
    }

    /// A format 4 `cmap` that maps U+0041 to glyph 1.
    fn cmap() -> Table {
        let mut table = Vec::new();
        for word in [0u16, 1, 3, 1, 0, 12] {
            table.extend_from_slice(&word.to_be_bytes());
        }
        let words: [u16; 16] = [4, 32, 0, 4, 4, 1, 0, 0x41, 0xFFFF, 0, 0x41, 0xFFFF, 1u16.wrapping_sub(0x41), 1, 0, 0];
        for word in words {
            table.extend_from_slice(&word.to_be_bytes());
        }
        (*b"cmap", table)
    }

    #[test]
    fn a_font_draws_what_it_maps_at_its_weight() {
        assert_eq!(drawn_weight(&[cmap(), os2(400)], 'A', 400), Some(400));
        assert_eq!(drawn_weight(&[cmap(), os2(100)], 'A', 900), Some(100));
        assert_eq!(drawn_weight(&[cmap()], 'A', 400), Some(400));
        assert_eq!(drawn_weight(&[cmap(), os2(400)], 'B', 400), None);
        assert_eq!(drawn_weight(&[os2(400)], 'A', 400), None);
    }

    #[test]
    fn a_variable_font_draws_at_the_weight_asked_for_within_its_range() {
        assert_eq!(drawn_weight(&[cmap(), os2(100), fvar(100, 900)], 'A', 400), Some(400));
        assert_eq!(drawn_weight(&[cmap(), os2(100), fvar(100, 900)], 'A', 700), Some(700));
        assert_eq!(drawn_weight(&[cmap(), os2(400), fvar(100, 900)], 'A', 50), Some(100));
        assert_eq!(drawn_weight(&[cmap(), os2(400), fvar(100, 200)], 'A', 400), Some(200));
        assert_eq!(drawn_weight(&[cmap(), os2(400), fvar(700, 900)], 'A', 400), Some(700));
    }

    /// A `name` table of Windows Unicode records of `names`, by name ID.
    fn name(names: &[(u16, &str)]) -> Table {
        let mut table = Vec::new();
        let header_len = 6 + 12 * names.len();
        for word in [0, names.len() as u16, header_len as u16] {
            table.extend_from_slice(&word.to_be_bytes());
        }
        let mut strings = Vec::new();
        for (id, text) in names {
            let text: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
            for word in [3, 1, 0x409, *id, text.len() as u16, strings.len() as u16] {
                table.extend_from_slice(&word.to_be_bytes());
            }
            strings.extend(text);
        }
        table.extend(strings);
        (*b"name", table)
    }

    #[test]
    fn a_fonts_family_is_its_typographic_family_else_its_family() {
        assert_eq!(family_name(&[name(&[(1, "Noto Serif"), (2, "Regular")])]).as_deref(), Some("Noto Serif"));
        let both = name(&[(1, "Noto Sans Display SemiBold"), (16, "Noto Sans Display")]);
        assert_eq!(family_name(&[both]).as_deref(), Some("Noto Sans Display"));
        assert_eq!(family_name(&[name(&[(2, "Regular")])]), None);
    }
}
