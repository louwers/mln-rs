// - face loading: src/sfnt/sfobjs.c (`sfnt_open_font`, `sfnt_load_face`:
//   table loading, face flags, charmaps), src/sfnt/ttload.c (`check_table_dir`,
//   `tt_face_lookup_table`), src/truetype/ttobjs.c (`tt_face_init` format
//   check), src/base/ftobjs.c (`FT_Open_Face` charmap selection);
// - sizing: src/base/ftobjs.c (`FT_Set_Char_Size`, `FT_Request_Metrics`),
//   src/truetype/ttdriver.c (`tt_size_request`);
// - advances: src/base/ftadvanc.c (`FT_Get_Advance`, fast path),
//   src/truetype/ttdriver.c (`tt_get_advances`), src/cff/cffdrivr.c
//   (`cff_get_advances`), src/sfnt/ttmtx.c (`tt_face_get_metrics`).
//
// Only what the C++ core reaches is ported: face index 0, unhinted advances
// at the default variation location. Comments starting "Not supported" name the rest.

use super::calc::{div_fix, mul_div};
use super::cmap::{self, Bytes, CharMap, Encoding, LookupContext};

const fn tag(t: [u8; 4]) -> u32 {
    u32::from_be_bytes(t)
}

/// A table directory entry (`TT_TableRec`).
#[derive(Clone, Copy, Debug)]
struct TableRec {
    tag: u32,
    offset: usize,
    length: usize,
}

/// The `FT_FaceRec` / `TT_FaceRec` fields the port needs.
#[derive(Clone, Debug)]
pub(crate) struct Face {
    pub(crate) format_tag: u32,
    tables: Vec<TableRec>,
    /// Units per EM: the `head` value for scalable and `sbix` faces, 0
    /// otherwise.
    pub(crate) units_per_em: u16,
    /// The `maxp` table's `numGlyphs`.
    pub(crate) num_glyphs: u32,
    /// `FT_FACE_FLAG_SCALABLE`.
    pub(crate) scalable: bool,
    /// `FT_FACE_FLAG_FIXED_SIZES`.
    pub(crate) fixed_sizes: bool,
    /// The CFF driver opened the face (an `OTTO` font).
    pub(crate) is_cff: bool,
    number_of_hmetrics: u16,
    hmtx: Option<(usize, usize)>,
    /// The `OS/2` table's version (0xFFFF when missing) and `fsSelection`.
    pub(crate) os2_version: u16,
    pub(crate) os2_fs_selection: u16,
    cmap: Option<(usize, usize)>,
    pub(crate) charmaps: Vec<CharMap>,
    /// The selected charmap, an index into `charmaps`.
    pub(crate) charmap: Option<usize>,
    /// The scale from font units to 26.6 pixels at the current size (16.16).
    pub(crate) x_scale: i64,
    pub(crate) y_scale: i64,
    /// FreeType would add a synthesized Unicode charmap (not ported).
    pub(crate) needs_synthesized_charmap: bool,
}

/// Why `FT_New_Memory_Face` failed (only the fact matters to the C++): the
/// name of the FreeType error, such as `Unknown_File_Format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("FT_New_Memory_Face failed: {0}")]
pub(crate) struct OpenError(pub(crate) &'static str);

impl Face {
    /// `FT_New_Memory_Face(library, data, size, 0, &face)`.
    pub(crate) fn open(data: &[u8]) -> Result<Face, OpenError> {
        let b = Bytes(data);
        // sfnt_open_font: a TrueType collection selects its first font.
        let mut sfnt_offset = 0usize;
        let mut format_tag = b.u32(0);
        if data.len() < 4 {
            return Err(OpenError("Unknown_File_Format"));
        }
        if format_tag == tag(*b"ttcf") {
            if data.len() < 16 || b.u32(8) == 0 {
                return Err(OpenError("Unknown_File_Format"));
            }
            sfnt_offset = b.u32(12) as usize;
            format_tag = b.u32(sfnt_offset);
        } else if format_tag == tag(*b"wOFF") || format_tag == tag(*b"wOF2") {
            // FreeType decodes these here (woff_open_font, woff2_open_font);
            // the port's callers decode them first, with crate::woff.
            return Err(OpenError("Unknown_File_Format (WOFF: decode it with crate::woff first)"));
        }
        // sfnt_open_font accepts these tags (plus Mac resource forks).
        let known = [tag(*b"\0\x01\0\0"), tag(*b"OTTO"), tag(*b"true"), tag(*b"\0\x02\0\0"), tag(*b"typ1")];
        if !known.contains(&format_tag) {
            return Err(OpenError("Unknown_File_Format"));
        }

        // tt_face_load_font_dir + check_table_dir
        let size = data.len();
        if sfnt_offset + 12 > size {
            return Err(OpenError("Invalid_Table"));
        }
        let num_tables = b.u16(sfnt_offset + 4) as usize;
        let mut tables = Vec::new();
        for nn in 0..num_tables {
            let p = sfnt_offset + 12 + nn * 16;
            if p + 16 > size {
                break;
            }
            let t = TableRec { tag: b.u32(p), offset: b.u32(p + 8) as usize, length: b.u32(p + 12) as usize };
            // We ignore invalid tables; `hmtx' and `vmtx' may be clipped.
            if t.offset > size {
                continue;
            }
            if t.length > size - t.offset {
                if t.tag == tag(*b"hmtx") || t.tag == tag(*b"vmtx") {
                    // tt_face_load_font_dir clips them to a multiple of 4.
                    tables.push(TableRec { length: (size - t.offset) & !3, ..t });
                }
                continue;
            }
            if (t.tag == tag(*b"head") || t.tag == tag(*b"bhed")) && t.length < 0x36 {
                return Err(OpenError("Table_Missing (head too small)"));
            }
            tables.push(t);
        }
        if tables.is_empty() {
            return Err(OpenError("Unknown_File_Format"));
        }

        // Driver selection (FT_Open_Face tries the TrueType driver first;
        // tt_face_init rejects other SFNT flavours, which the CFF driver
        // takes if they have CFF data).
        let is_tt = [tag(*b"\0\x01\0\0"), tag(*b"\0\x02\0\0"), tag(*b"true")].contains(&format_tag);
        let mut face = Face {
            format_tag,
            tables,
            units_per_em: 0,
            num_glyphs: 0,
            scalable: false,
            fixed_sizes: false,
            is_cff: !is_tt,
            number_of_hmetrics: 0,
            hmtx: None,
            os2_version: 0xFFFF,
            os2_fs_selection: 0,
            cmap: None,
            charmaps: Vec::new(),
            charmap: None,
            x_scale: 0,
            y_scale: 0,
            needs_synthesized_charmap: false,
        };
        if !is_tt && face.lookup(tag(*b"CFF ")).is_none() && face.lookup(tag(*b"CFF2")).is_none() {
            // Not supported: Type 1 in SFNT ('typ1'), pure CFF/CID fonts.
            return Err(OpenError("Unknown_File_Format (no TrueType or CFF data)"));
        }
        face.load(data)?;
        Ok(face)
    }

    /// `tt_face_lookup_table`: the first entry with that tag; zero-length
    /// tables count as missing.
    fn lookup(&self, t: u32) -> Option<TableRec> {
        self.tables.iter().find(|e| e.tag == t && e.length != 0).copied()
    }

    /// Every tag of the table directory with what `tt_face_lookup_table`
    /// finds for it.
    #[must_use]
    pub(crate) fn table_entries(&self) -> Vec<([u8; 4], (usize, usize))> {
        let mut out: Vec<([u8; 4], (usize, usize))> = Vec::new();
        for e in &self.tables {
            let t = e.tag.to_be_bytes();
            if out.iter().all(|(o, _)| *o != t)
                && let Some(found) = self.lookup(e.tag)
            {
                out.push((t, (found.offset, found.length)));
            }
        }
        out
    }

    /// `sfnt_load_face` (the parts with an effect here).
    fn load(&mut self, data: &[u8]) -> Result<(), OpenError> {
        let b = Bytes(data);
        let has_outline_tables = self.lookup(tag(*b"glyf")).is_some()
            || self.lookup(tag(*b"CFF ")).is_some()
            || self.lookup(tag(*b"CFF2")).is_some();
        let has_cblc = self.lookup(tag(*b"CBLC")).is_some();
        let has_cbdt = self.lookup(tag(*b"CBDT")).is_some();
        let has_eblc = self.lookup(tag(*b"EBLC")).is_some();
        let has_bloc = self.lookup(tag(*b"bloc")).is_some();
        let has_sbix = self.lookup(tag(*b"sbix")).is_some();
        let mut has_outline = has_outline_tables;

        // Not supported: Apple `bhed' bitmap-only fonts.
        let head = self.lookup(tag(*b"head")).ok_or(OpenError("Table_Missing (head)"))?;
        // Ignore outlines for CBLC/CBDT fonts.
        if has_cblc || has_cbdt {
            has_outline = false;
        }
        let head_units_per_em = b.u16(head.offset + 18) as u16;
        if !(16..=16384).contains(&head_units_per_em) {
            return Err(OpenError("Invalid_Table (units per em)"));
        }

        // maxp (errors ignored: numGlyphs stays 0).
        // The CFF driver takes the glyph count from the CFF
        // CharStrings INDEX (CID fonts: max CID + 1), not from maxp; the two
        // agree in every font of //difftest/text.
        if let Some(maxp) = self.lookup(tag(*b"maxp"))
            && maxp.length >= 6
        {
            self.num_glyphs = b.u16(maxp.offset + 4);
        }
        // cmap (tt_face_load_cmap: the whole table is extracted).
        self.cmap = self.lookup(tag(*b"cmap")).map(|t| (t.offset, t.length));

        // hhea + hmtx (not optional, except for Mac `true' fonts).
        match self.lookup(tag(*b"hhea")) {
            Some(hhea) => {
                if hhea.length < 36 {
                    return Err(OpenError("Invalid_Table (hhea)"));
                }
                self.number_of_hmetrics = b.u16(hhea.offset + 34) as u16;
                let hmtx = self.lookup(tag(*b"hmtx")).ok_or(OpenError("Hmtx_Table_Missing"))?;
                self.hmtx = Some((hmtx.offset, hmtx.length));
            }
            None => {
                if self.format_tag == tag(*b"true") {
                    has_outline = false;
                } else {
                    return Err(OpenError("Horiz_Header_Missing"));
                }
            }
        }

        // OS/2 (treated as missing on any error).
        if let Some(os2) = self.lookup(tag(*b"OS/2")) {
            // tt_face_load_os2 needs the 78-byte version 0 table at least.
            if os2.length >= 78 {
                self.os2_version = b.u16(os2.offset) as u16;
                self.os2_fs_selection = b.u16(os2.offset + 62) as u16;
            }
        }

        // Embedded bitmap strikes (TT_CONFIG_OPTION_EMBEDDED_BITMAPS).
        if has_cblc || has_eblc || has_bloc || has_sbix {
            self.fixed_sizes = self.has_valid_strikes(data, has_cblc, has_eblc, has_bloc, has_sbix);
        }

        // Face flags.
        if has_outline {
            // By default fonts with an `sbix' table are bitmap-only.
            if !has_sbix {
                self.scalable = true;
            }
        }
        // A font with no bitmaps and no outlines is scalable.
        if !self.fixed_sizes && !self.scalable {
            self.scalable = true;
        }
        // Set up metrics: `root->units_per_EM` stays 0 for bitmap-only
        // (e.g. CBDT) faces.
        let sbix_flag = has_outline && has_sbix; // FT_FACE_FLAG_SBIX
        if self.scalable || sbix_flag {
            self.units_per_em = head_units_per_em;
        }

        // Charmaps.
        self.charmaps = cmap::build_cmaps(data, self.cmap);
        let has_unicode =
            self.charmaps.iter().any(|cm| cm.encoding == Encoding::Unicode || cm.encoding == Encoding::MsSymbol);
        // Not supported: without a Unicode (or symbol) charmap FreeType
        // synthesizes one from the `post' glyph names here
        // (FT_CONFIG_OPTION_POSTSCRIPT_NAMES), and the CFF driver one from
        // the CFF glyph names when there is no (3,1) or (0,x) charmap.
        self.needs_synthesized_charmap = !has_unicode
            || (self.is_cff
                && !self.charmaps.iter().any(|cm| (cm.platform_id == 3 && cm.encoding_id == 1) || cm.platform_id == 0));

        // The CFF driver appends charmaps for the CFF font's
        // Standard/Expert/Custom encodings (platform 7); force_ucs2_charmap
        // and find_unicode_charmap never select them.

        // Not supported: tt_check_single_notdef (sbit fonts whose only outline
        // is .notdef lose FT_FACE_FLAG_SCALABLE).

        // FT_Open_Face: select Unicode charmap by default.
        self.charmap = cmap::find_unicode_charmap(&self.charmaps);
        Ok(())
    }

    /// Whether `tt_face_load_sbit` + `load_strike_metrics` find at least one
    /// strike with a non-zero ppem.
    fn has_valid_strikes(&self, data: &[u8], cblc: bool, eblc: bool, bloc: bool, sbix: bool) -> bool {
        let b = Bytes(data);
        // tt_face_load_sbit prefers EBLC, then CBLC, then bloc, then sbix.
        let bitmap_location = if eblc {
            self.lookup(tag(*b"EBLC"))
        } else if cblc {
            self.lookup(tag(*b"CBLC"))
        } else if bloc {
            self.lookup(tag(*b"bloc"))
        } else {
            None
        };
        if let Some(t) = bitmap_location {
            if t.length < 8 {
                return false;
            }
            let version = b.u32(t.offset);
            let num_strikes = b.u32(t.offset + 4) as usize;
            if !(0x0002_0000..=0x0003_0000).contains(&version) || num_strikes >= 0x10000 {
                return false;
            }
            // BitmapSize records are 48 bytes; ppemX/ppemY at 44/45.
            if 8 + num_strikes * 48 > t.length {
                return false;
            }
            return (0..num_strikes).any(|i| {
                let p = t.offset + 8 + i * 48;
                b.u8(p + 44) != 0 && b.u8(p + 45) != 0
            });
        }
        if sbix {
            let Some(t) = self.lookup(tag(*b"sbix")) else { return false };
            if t.length < 8 {
                return false;
            }
            let num_strikes = b.u32(t.offset + 4) as usize;
            return (0..num_strikes).any(|i| {
                let off = b.u32(t.offset + 8 + i * 4) as usize;
                off + 4 <= t.length && b.u16(t.offset + off) != 0
            });
        }
        false
    }

    /// The lookup context of `FT_Get_Char_Index` on this face.
    #[must_use]
    pub(crate) fn lookup_context<'a>(&self, data: &'a [u8]) -> LookupContext<'a> {
        LookupContext { data, cmap_limit: self.cmap.map_or(0, |(o, s)| o + s), num_glyphs: self.num_glyphs }
    }

    /// `FT_Set_Charmap`.
    pub(crate) fn set_charmap(&mut self, index: usize) -> Result<(), &'static str> {
        if self.charmaps[index].format == 14 {
            Err("Invalid_Argument")
        } else {
            self.charmap = Some(index);
            Ok(())
        }
    }

    /// `FT_Set_Char_Size(face, 0, char_height, 72, 72)` with the C++ core's
    /// arguments: sets `size->metrics.x_scale`/`y_scale`.
    pub(crate) fn set_char_size(&mut self, char_height: i64, resolution: i64) {
        // FT_Set_Char_Size: width 0 means "same as height".
        let char_width = char_height.max(64);
        let char_height = char_height.max(64);
        // tt_size_request / cff_size_request first try an embedded bitmap
        // strike of exactly this size (FT_Match_Size). For a scalable face a
        // matching strike gives FT_DivFix(ppem << 6, upem), the value
        // FT_Request_Metrics computes below for a nominal request; for a
        // bitmap-only face both give 1.0.
        // Not supported: a strike whose x ppem differs from its (matching) y ppem.
        if self.scalable {
            // FT_Request_Metrics, FT_SIZE_REQUEST_TYPE_NOMINAL. (tt_size_reset
            // rounds the ppem only in `hinted_metrics`; FT_Get_Advance and
            // hb-ft read the unrounded `size->metrics`.)
            let w = i64::from(self.units_per_em);
            let h = w;
            let scaled_w = (char_width * resolution + 36) / 72; // FT_REQUEST_WIDTH
            let scaled_h = (char_height * resolution + 36) / 72;
            self.y_scale = div_fix(scaled_h, h);
            self.x_scale = div_fix(scaled_w, w);
        } else {
            self.x_scale = 1 << 16;
            self.y_scale = 1 << 16;
        }
    }

    /// `FT_Get_Advance(face, gindex, FT_LOAD_DEFAULT | FT_LOAD_NO_HINTING)`:
    /// the 16.16 advance, or an error (the caller keeps 0).
    #[must_use]
    pub(crate) fn get_advance(&self, data: &[u8], gindex: u32) -> Option<i64> {
        if gindex >= self.num_glyphs {
            return None; // Invalid_Glyph_Index
        }
        // tt_get_advances / cff_get_advances at the default location: the
        // `hmtx' advance.
        if self.is_cff && self.number_of_hmetrics == 0 {
            // Not supported: cff_get_advances' Missing_Table path loads the
            // glyph for the CFF charstring width.
            return None;
        }
        let aw = self.get_hmetrics_advance(data, gindex);
        // ft_face_scale_advances_
        Some(mul_div(i64::from(aw), self.x_scale, 64))
    }

    /// `tt_face_get_metrics(face, 0, gindex, &lsb, &aw)`'s advance.
    fn get_hmetrics_advance(&self, data: &[u8], gindex: u32) -> u16 {
        let b = Bytes(data);
        let Some((table_pos, table_size)) = self.hmtx else {
            return 0;
        };
        let table_end = table_pos + table_size;
        let k = u32::from(self.number_of_hmetrics);
        if k == 0 {
            return 0;
        }
        let pos = if gindex < k { table_pos + 4 * gindex as usize } else { table_pos + 4 * (k as usize - 1) };
        if pos + 2 > table_end || pos + 2 > data.len() {
            return 0;
        }
        if gindex < k && pos + 4 > table_end {
            return 0;
        }
        b.u16(pos) as u16
    }
}
