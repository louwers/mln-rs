//! Where a system font's bytes come from, whether the map can draw a script
//! with it, and its bytes, mapped the first time the map needs them.

#![expect(unsafe_code, reason = "memory-mapping a font file is unsafe")]

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use memmap2::Mmap;
use mln_text::glyph::FontData;
use mln_util::logging::{Event, Log};
use skrifa::raw::tables::cmap::Cmap;
use skrifa::raw::tables::fvar::Fvar;
use skrifa::raw::tables::os2::Os2;
use skrifa::raw::{FontData as RawData, FontRead};

use crate::sfnt::{self, Table};

/// A font that the platform found.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    /// Face `index` of the font file at `path`.
    File { path: PathBuf, index: u32 },
}

impl Source {
    /// A url that names the font, unique to it.
    pub(crate) fn url(&self) -> String {
        match self {
            Source::File { path, index: 0 } => format!("file://{}", path.display()),
            Source::File { path, index } => format!("file://{}#{index}", path.display()),
        }
    }

    /// The font's tables `tags`, those it has.
    fn tables(&self, tags: &[[u8; 4]]) -> Option<Vec<Table>> {
        match self {
            Source::File { path, index } => sfnt::read_tables(path, *index, tags).ok(),
        }
    }

    /// The weight the map draws the font at, if it has a glyph for `probe`.
    pub(crate) fn drawn_weight(&self, probe: char) -> Option<u16> {
        drawn_weight(&self.tables(&[*b"cmap", *b"OS/2", *b"fvar"])?, probe)
    }
}

/// The font files of the chosen fonts, each shared by the faces that use it.
#[derive(Default)]
pub(crate) struct Files(HashMap<PathBuf, Arc<LazyFile>>);

impl Files {
    /// The bytes of `source`'s font, mapped the first time the map needs
    /// them.
    pub(crate) fn data(&mut self, source: &Source) -> FontData {
        match source {
            Source::File { path, index } => {
                let file = self
                    .0
                    .entry(path.clone())
                    .or_insert_with(|| Arc::new(LazyFile { path: path.clone(), map: OnceLock::new() }));
                let file: Arc<LazyFile> = Arc::clone(file);
                FontData::new(file).with_face_index(*index)
            }
        }
    }
}

/// Whether a font that draws at `weight` is near regular. Every label in a
/// fallback font's script draws at the font's one weight, whatever the font
/// stack asks for.
pub(crate) fn near_regular(weight: u16) -> bool {
    (300..=600).contains(&weight)
}

/// The weight the map draws a font with `tables` at, if it has a glyph for
/// `probe`.
///
/// The map's font engine has no font variations, so a variable font draws
/// at its default instance: the default of its `wght` axis, which can be far
/// from its family's regular weight (Fedora's Noto Sans CJK variable font
/// defaults to Thin, 100).
pub(crate) fn drawn_weight(tables: &[Table], probe: char) -> Option<u16> {
    let table = |tag: &[u8; 4]| tables.iter().find(|(t, _)| t == tag).map(|(_, data)| RawData::new(data));
    let cmap = Cmap::read(table(b"cmap")?).ok()?;
    cmap.map_codepoint(probe)?;
    let wght = table(b"fvar").and_then(|data| Fvar::read(data).ok()).and_then(|fvar| {
        let axes = fvar.axes().ok()?;
        let axis = axes.iter().find(|axis| axis.axis_tag() == skrifa::Tag::new(b"wght"))?;
        Some(axis.default_value().to_f64().round() as u16)
    });
    Some(
        wght.or_else(|| table(b"OS/2").and_then(|data| Os2::read(data).ok()).map(|os2| os2.us_weight_class()))
            .unwrap_or(400),
    )
}

/// A font file, mapped the first time the map needs it. A map leaves the
/// pages the map never reads on disk: a CJK font is tens of megabytes, and a
/// map's labels use a few thousand of its glyphs.
struct LazyFile {
    path: PathBuf,
    map: OnceLock<Option<Mmap>>,
}

impl AsRef<[u8]> for LazyFile {
    fn as_ref(&self) -> &[u8] {
        self.map.get_or_init(|| map(&self.path)).as_deref().unwrap_or_default()
    }
}

/// The file at `path`, memory-mapped.
fn map(path: &Path) -> Option<Mmap> {
    let file = File::open(path)
        .inspect_err(|error| Log::warning(Event::Glyph, format!("Couldn't open {}: {error}", path.display())))
        .ok()?;
    // SAFETY: the mapping is read-only, and system font files aren't changed
    // in place: an update replaces a font by renaming a new file over it,
    // which leaves the mapped file as it was.
    unsafe { Mmap::map(&file) }
        .inspect_err(|error| Log::warning(Event::Glyph, format!("Couldn't map {}: {error}", path.display())))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::{drawn_weight, near_regular};
    use crate::sfnt::Table;

    fn os2(weight: u16) -> Table {
        let mut table = vec![0; 78];
        table[4..6].copy_from_slice(&weight.to_be_bytes());
        (*b"OS/2", table)
    }

    /// An `fvar` with a `wght` axis that defaults to `default`.
    fn fvar(default: u16) -> Table {
        let mut table = Vec::new();
        // version 1.0, axesArrayOffset 16, reserved, axisCount 1, axisSize
        // 20, instanceCount 0, instanceSize 8.
        for word in [1u16, 0, 16, 2, 1, 20, 0, 8] {
            table.extend_from_slice(&word.to_be_bytes());
        }
        table.extend_from_slice(b"wght");
        for value in [100u16, default, 900] {
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
        assert_eq!(drawn_weight(&[cmap(), os2(400)], 'A'), Some(400));
        assert_eq!(drawn_weight(&[cmap(), os2(100)], 'A'), Some(100));
        assert_eq!(drawn_weight(&[cmap()], 'A'), Some(400));
        assert_eq!(drawn_weight(&[cmap(), os2(400)], 'B'), None);
        assert_eq!(drawn_weight(&[os2(400)], 'A'), None);
    }

    #[test]
    fn a_variable_font_draws_at_its_default_weight() {
        assert_eq!(drawn_weight(&[cmap(), os2(400), fvar(100)], 'A'), Some(100));
        assert_eq!(drawn_weight(&[cmap(), os2(100), fvar(400)], 'A'), Some(400));
    }

    #[test]
    fn regular_is_300_to_600() {
        assert!(!near_regular(200));
        assert!(near_regular(300));
        assert!(near_regular(600));
        assert!(!near_regular(700));
    }
}
