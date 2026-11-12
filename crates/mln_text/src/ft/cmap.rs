// Ports the character-map parts of ../maplibre-native/vendor/freetype (2.13):
// src/sfnt/ttcmap.c (`tt_face_build_cmaps`, the `tt_cmapN_validate`
// functions at FT_VALIDATE_DEFAULT and the `tt_cmapN_char_index` lookups,
// `tt_cmap14_char_var_index`), src/sfnt/sfobjs.c (`sfnt_find_encoding`) and
// src/base/ftobjs.c (`find_unicode_charmap`, `find_variant_selector_charmap`,
// `FT_Set_Charmap`, `FT_Get_Char_Index`, `FT_Face_GetCharVariantIndex`).
//
// The C code reads the cmap through pointers into the font file; reads the
// C++ does past the end of the file (undefined behaviour) read as 0 here.

use std::cmp::Ordering;

/// `FT_Encoding` values that matter here (`sfnt_find_encoding`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoding {
    None,
    Unicode,
    MsSymbol,
    AppleRoman,
    Sjis,
    Prc,
    Big5,
    Wansung,
    Johab,
}

/// `sfnt_find_encoding`.
#[must_use]
pub fn sfnt_find_encoding(platform_id: u16, encoding_id: u16) -> Encoding {
    match (platform_id, encoding_id) {
        // TT_PLATFORM_ISO, TT_PLATFORM_APPLE_UNICODE, TT_MS_ID_UCS_4 and
        // TT_MS_ID_UNICODE_CS.
        (2 | 0, _) | (3, 10 | 1) => Encoding::Unicode,
        (1, 0) => Encoding::AppleRoman, // TT_MAC_ID_ROMAN
        (3, 0) => Encoding::MsSymbol,   // TT_MS_ID_SYMBOL_CS
        (3, 2) => Encoding::Sjis,
        (3, 3) => Encoding::Prc,
        (3, 4) => Encoding::Big5,
        (3, 5) => Encoding::Wansung,
        (3, 6) => Encoding::Johab,
        _ => Encoding::None,
    }
}

/// `TT_CMAP_FLAG_UNSORTED` / `TT_CMAP_FLAG_OVERLAPPING` (format 4).
const TT_CMAP_FLAG_UNSORTED: i32 = 1;
const TT_CMAP_FLAG_OVERLAPPING: i32 = 2;

/// One `FT_CharMap` of a face, built by `tt_face_build_cmaps`.
#[derive(Clone, Debug)]
pub struct CharMap {
    pub platform_id: u16,
    pub encoding_id: u16,
    pub encoding: Encoding,
    pub format: u16,
    /// Absolute offset of the subtable in the font file (`ttcmap->data`).
    pub data: usize,
    /// `TT_CMap::flags` (the validator's return value).
    pub flags: i32,
}

/// Big-endian reads at absolute offsets of the font file.
#[derive(Clone, Copy)]
pub struct Bytes<'a>(pub &'a [u8]);

impl Bytes<'_> {
    #[inline]
    #[must_use]
    pub fn u8(&self, p: usize) -> u32 {
        u32::from(self.0.get(p).copied().unwrap_or(0))
    }
    #[inline]
    #[must_use]
    pub fn u16(&self, p: usize) -> u32 {
        (self.u8(p) << 8) | self.u8(p.wrapping_add(1))
    }
    #[inline]
    #[must_use]
    pub fn i16(&self, p: usize) -> i32 {
        i32::from(self.u16(p) as u16 as i16)
    }
    #[inline]
    #[must_use]
    pub fn u24(&self, p: usize) -> u32 {
        (self.u8(p) << 16) | (self.u8(p.wrapping_add(1)) << 8) | self.u8(p.wrapping_add(2))
    }
    #[inline]
    #[must_use]
    pub fn u32(&self, p: usize) -> u32 {
        (self.u16(p) << 16) | self.u16(p.wrapping_add(2))
    }
}

/// `FT_INVALID_*`: the validator longjmps out; the subtable is ignored.
struct Invalid;

type Validation = Result<i32, Invalid>;

/// `tt_cmap0_validate`.
fn validate0(b: Bytes, table: usize, limit: usize) -> Validation {
    if table + 2 + 2 > limit {
        return Err(Invalid);
    }
    let length = b.u16(table + 2) as usize;
    if table + length > limit || length < 262 {
        return Err(Invalid);
    }
    Ok(0)
}

/// `tt_cmap2_validate`.
fn validate2(b: Bytes, table: usize, limit: usize) -> Validation {
    if table + 2 + 2 > limit {
        return Err(Invalid);
    }
    let length = b.u16(table + 2) as usize;
    if table + length > limit || length < 6 + 512 {
        return Err(Invalid);
    }
    let keys = table + 6;
    let mut max_subs = 0u32;
    for n in 0..256 {
        let idx = b.u16(keys + n * 2) >> 3;
        if idx > max_subs {
            max_subs = idx;
        }
    }
    let subs = table + 518;
    let glyph_ids = subs + (max_subs as usize + 1) * 8;
    if glyph_ids > limit {
        return Err(Invalid);
    }
    let mut p = subs;
    for _ in 0..=max_subs {
        let _first_code = b.u16(p);
        let code_count = b.u16(p + 2) as usize;
        let offset = b.u16(p + 6) as usize;
        p += 8;
        if code_count == 0 {
            continue;
        }
        if offset != 0 {
            let ids = p - 2 + offset;
            if ids < glyph_ids || ids + code_count * 2 > table + length {
                return Err(Invalid);
            }
        }
    }
    Ok(0)
}

/// `tt_cmap4_validate`.
fn validate4(b: Bytes, table: usize, limit: usize) -> Validation {
    if table + 2 + 2 > limit {
        return Err(Invalid);
    }
    let mut length = b.u16(table + 2) as usize;
    // In certain fonts, the `length' field is invalid and goes out of
    // bound; it also happens that it is too small. Both are corrected at the
    // default validation level.
    if table + length > limit {
        length = limit - table;
    }
    if length < limit - table {
        length = limit - table;
    }
    if length < 16 {
        return Err(Invalid);
    }
    let num_segs = (b.u16(table + 6) / 2) as usize;
    if length < 16 + num_segs * 2 * 4 {
        return Err(Invalid);
    }
    let ends = table + 14;
    let starts = table + 16 + num_segs * 2;
    let deltas = starts + num_segs * 2;
    let offsets = deltas + num_segs * 2;
    let glyph_ids = offsets + num_segs * 2;

    let mut error = 0;
    let (mut last_start, mut last_end) = (0u32, 0u32);
    for n in 0..num_segs {
        let p = offsets + n * 2;
        let start = b.u16(starts + n * 2);
        let end = b.u16(ends + n * 2);
        let offset = b.u16(offsets + n * 2) as usize;
        if start > end {
            return Err(Invalid);
        }
        // Overlapping ranges are allowed at the default level.
        if start <= last_end && n > 0 {
            if last_start > start || last_end > end {
                error |= TT_CMAP_FLAG_UNSORTED;
            } else {
                error |= TT_CMAP_FLAG_OVERLAPPING;
            }
        }
        if offset != 0 && offset != 0xFFFF {
            let p = p + offset; // start of glyph ID array
            if (n != num_segs - 1 || !(start == 0xFFFF && end == 0xFFFF))
                && (p < glyph_ids || p + (end - start + 1) as usize * 2 > limit)
            {
                return Err(Invalid);
            }
        } else if offset == 0xFFFF {
            // Some fonts (erroneously?) use a range offset of 0xFFFF to
            // mean missing glyph in cmap table.
            if n != num_segs - 1 || !(start == 0xFFFF && end == 0xFFFF) {
                return Err(Invalid);
            }
        }
        last_start = start;
        last_end = end;
    }
    Ok(error)
}

/// `tt_cmap6_validate`.
fn validate6(b: Bytes, table: usize, limit: usize) -> Validation {
    if table + 10 > limit {
        return Err(Invalid);
    }
    let length = b.u16(table + 2) as usize;
    let count = b.u16(table + 8) as usize;
    if table + length > limit || length < 10 + count * 2 {
        return Err(Invalid);
    }
    Ok(0)
}

/// `tt_cmap8_validate` (default level).
fn validate8(b: Bytes, table: usize, limit: usize) -> Validation {
    if table + 16 + 8192 > limit {
        return Err(Invalid);
    }
    let length = b.u32(table + 4) as usize;
    if length > limit - table || length < 8192 + 16 {
        return Err(Invalid);
    }
    let mut p = table + 12 + 8192;
    let num_groups = b.u32(p) as usize;
    p += 4;
    if num_groups > (limit - p) / 12 {
        return Err(Invalid);
    }
    let mut last = 0u32;
    for n in 0..num_groups {
        let start = b.u32(p);
        let end = b.u32(p + 4);
        p += 12;
        if start > end {
            return Err(Invalid);
        }
        if n > 0 && start <= last {
            return Err(Invalid);
        }
        last = end;
    }
    Ok(0)
}

/// `tt_cmap10_validate`.
fn validate10(b: Bytes, table: usize, limit: usize) -> Validation {
    if table + 20 > limit {
        return Err(Invalid);
    }
    let length = b.u32(table + 4) as usize;
    let count = b.u32(table + 16) as usize;
    if length > limit - table || length < 20 || (length - 20) / 2 < count {
        return Err(Invalid);
    }
    Ok(0)
}

/// `tt_cmap12_validate` and `tt_cmap13_validate` (identical at the
/// default level).
fn validate12(b: Bytes, table: usize, limit: usize) -> Validation {
    if table + 16 > limit {
        return Err(Invalid);
    }
    let length = b.u32(table + 4) as usize;
    let num_groups = b.u32(table + 12) as usize;
    if length > limit - table || length < 16 || (length - 16) / 12 < num_groups {
        return Err(Invalid);
    }
    let mut p = table + 16;
    let mut last = 0u32;
    for n in 0..num_groups {
        let start = b.u32(p);
        let end = b.u32(p + 4);
        p += 12;
        if start > end {
            return Err(Invalid);
        }
        if n > 0 && start <= last {
            return Err(Invalid);
        }
        last = end;
    }
    Ok(0)
}

/// `tt_cmap14_validate`.
fn validate14(b: Bytes, table: usize, limit: usize) -> Validation {
    if table + 2 + 4 + 4 > limit {
        return Err(Invalid);
    }
    let length = b.u32(table + 2) as usize;
    let num_selectors = b.u32(table + 6) as usize;
    if length > limit - table || length < 10 || (length - 10) / 11 < num_selectors {
        return Err(Invalid);
    }
    let mut p = table + 10;
    let mut last_var_sel = 1u32;
    for _ in 0..num_selectors {
        let var_sel = b.u24(p);
        let def_off = b.u32(p + 3) as usize;
        let nondef_off = b.u32(p + 7) as usize;
        p += 11;
        if def_off >= length || nondef_off >= length {
            return Err(Invalid);
        }
        if var_sel < last_var_sel {
            return Err(Invalid);
        }
        last_var_sel = var_sel + 1;
        if def_off != 0 {
            let mut defp = table + def_off;
            if defp + 4 > limit {
                return Err(Invalid);
            }
            let num_ranges = b.u32(defp) as usize;
            defp += 4;
            if num_ranges > (limit - defp) / 4 {
                return Err(Invalid);
            }
            let mut last_base = 0u32;
            for _ in 0..num_ranges {
                let base = b.u24(defp);
                let cnt = b.u8(defp + 3);
                defp += 4;
                if base + cnt >= 0x0011_0000 {
                    return Err(Invalid);
                }
                if base < last_base {
                    return Err(Invalid);
                }
                last_base = base + cnt + 1;
            }
        }
        if nondef_off != 0 {
            let mut ndp = table + nondef_off;
            if ndp + 4 > limit {
                return Err(Invalid);
            }
            let num_mappings = b.u32(ndp) as usize;
            ndp += 4;
            if num_mappings > (limit - ndp) / 5 {
                return Err(Invalid);
            }
            let mut last_uni = 0u32;
            for _ in 0..num_mappings {
                let uni = b.u24(ndp);
                ndp += 5;
                if uni >= 0x0011_0000 {
                    return Err(Invalid);
                }
                if uni < last_uni {
                    return Err(Invalid);
                }
                last_uni = uni + 1;
            }
        }
    }
    Ok(0)
}

/// `tt_face_build_cmaps`: the charmaps of a face, in encoding-record order,
/// for every subtable of a supported format that validates.
/// `cmap` is the (offset, size) of the `cmap` table, if loaded.
#[must_use]
pub fn build_cmaps(data: &[u8], cmap: Option<(usize, usize)>) -> Vec<CharMap> {
    let mut charmaps = Vec::new();
    let Some((table, cmap_size)) = cmap else {
        return charmaps;
    };
    let b = Bytes(data);
    if cmap_size < 4 {
        return charmaps;
    }
    let limit = table + cmap_size;
    let mut p = table + 2;
    let mut num_cmaps = b.u16(p);
    p += 2;
    while num_cmaps > 0 && p + 8 <= limit {
        let platform_id = b.u16(p) as u16;
        let encoding_id = b.u16(p + 2) as u16;
        let offset = b.u32(p + 4) as usize;
        p += 8;
        num_cmaps -= 1;

        if offset != 0 && offset <= cmap_size - 2 {
            let sub = table + offset;
            let format = b.u16(sub) as u16;
            let validation = match format {
                0 => validate0(b, sub, limit),
                2 => validate2(b, sub, limit),
                4 => validate4(b, sub, limit),
                6 => validate6(b, sub, limit),
                8 => validate8(b, sub, limit),
                10 => validate10(b, sub, limit),
                12 | 13 => validate12(b, sub, limit),
                14 => validate14(b, sub, limit),
                // "unsupported cmap sub-table ignored"
                _ => continue,
            };
            if let Ok(flags) = validation {
                charmaps.push(CharMap {
                    platform_id,
                    encoding_id,
                    // Filled in by sfnt_load_face right after.
                    encoding: sfnt_find_encoding(platform_id, encoding_id),
                    format,
                    data: sub,
                    flags,
                });
            }
            // else: "broken cmap sub-table ignored"
        }
    }
    charmaps
}

/// `find_unicode_charmap`: the last UCS-4 charmap, else the last Unicode
/// one (which can be the format 14 subtable: FreeType does not skip it).
#[must_use]
pub fn find_unicode_charmap(charmaps: &[CharMap]) -> Option<usize> {
    for (i, cm) in charmaps.iter().enumerate().rev() {
        if cm.encoding == Encoding::Unicode
            && ((cm.platform_id == 3 && cm.encoding_id == 10) || (cm.platform_id == 0 && cm.encoding_id == 4))
        {
            return Some(i);
        }
    }
    charmaps.iter().rposition(|cm| cm.encoding == Encoding::Unicode)
}

/// `find_variant_selector_charmap`.
#[must_use]
pub fn find_variant_selector_charmap(charmaps: &[CharMap]) -> Option<usize> {
    charmaps.iter().position(|cm| cm.platform_id == 0 && cm.encoding_id == 5 && cm.format == 14)
}

/// The face state a lookup needs: the file, the cmap table end and
/// `face->root.num_glyphs`.
#[derive(Clone, Copy)]
pub struct LookupContext<'a> {
    pub data: &'a [u8],
    /// `face->cmap_table + face->cmap_size`.
    pub cmap_limit: usize,
    pub num_glyphs: u32,
}

/// `cmap->clazz->char_index`.
#[must_use]
pub fn char_index(ctx: LookupContext, cm: &CharMap, char_code: u32) -> u32 {
    let b = Bytes(ctx.data);
    let table = cm.data;
    match cm.format {
        // tt_cmap0_char_index
        0 => {
            if char_code < 256 {
                b.u8(table + 6 + char_code as usize)
            } else {
                0
            }
        }
        2 => cmap2_char_index(b, table, char_code),
        4 => {
            if char_code >= 0x10000 {
                return 0;
            }
            if cm.flags & TT_CMAP_FLAG_UNSORTED != 0 {
                cmap4_char_map_linear(ctx, table, char_code)
            } else {
                cmap4_char_map_binary(ctx, cm, table, char_code)
            }
        }
        // tt_cmap6_char_index
        6 => {
            let start = b.u16(table + 6);
            let count = b.u16(table + 8);
            let idx = char_code.wrapping_sub(start);
            if idx < count { b.u16(table + 10 + 2 * idx as usize) } else { 0 }
        }
        // tt_cmap8_char_index
        8 => {
            let mut p = table + 8204;
            let mut num_groups = b.u32(p);
            p += 4;
            while num_groups > 0 {
                let start = b.u32(p);
                let end = b.u32(p + 4);
                let start_id = b.u32(p + 8);
                p += 12;
                num_groups -= 1;
                if char_code < start {
                    break;
                }
                if char_code <= end {
                    if start_id > 0xFFFF_FFFF - (char_code - start) {
                        return 0;
                    }
                    return start_id + (char_code - start);
                }
            }
            0
        }
        // tt_cmap10_char_index
        10 => {
            let start = b.u32(table + 12);
            let count = b.u32(table + 16);
            if char_code < start {
                return 0;
            }
            let idx = char_code - start;
            if idx < count { b.u16(table + 20 + 2 * idx as usize) } else { 0 }
        }
        12 => cmap12_char_map_binary(b, table, char_code, false),
        13 => cmap12_char_map_binary(b, table, char_code, true),
        // tt_cmap14_char_index: "This can't happen"
        _ => 0,
    }
}

/// `tt_cmap2_get_subheader` + `tt_cmap2_char_index`.
fn cmap2_char_index(b: Bytes, table: usize, char_code: u32) -> u32 {
    if char_code >= 0x10000 {
        return 0;
    }
    let char_lo = (char_code & 0xFF) as usize;
    let char_hi = (char_code >> 8) as usize;
    let keys = table + 6;
    let subs = table + 518;
    let sub = if char_hi == 0 {
        // An 8-bit character code: subHeader 0, only if its key is 0.
        if b.u16(keys + char_lo * 2) != 0 {
            return 0;
        }
        subs
    } else {
        let sub = subs + (b.u16(keys + char_hi * 2) as usize & !7);
        if sub == subs {
            return 0;
        }
        sub
    };
    let start = b.u16(sub);
    let count = b.u16(sub + 2);
    let delta = b.i16(sub + 4);
    let offset = b.u16(sub + 6) as usize;
    let idx = (char_lo as u32).wrapping_sub(start);
    if idx < count && offset != 0 {
        let gid = b.u16(sub + 6 + offset + 2 * idx as usize);
        if gid != 0 {
            return ((gid as i32 + delta) as u32) & 0xFFFF;
        }
    }
    0
}

/// `tt_cmap4_char_map_linear` with `next = 0`.
fn cmap4_char_map_linear(ctx: LookupContext, table: usize, charcode: u32) -> u32 {
    let b = Bytes(ctx.data);
    let num_segs = (b.u16(table + 6) >> 1) as usize;
    if num_segs == 0 {
        return 0;
    }
    let num_segs2 = num_segs << 1;
    let mut p = table + 14; // ends
    let mut q = table + 16 + num_segs2; // starts
    let mut gindex = 0;
    for i in 0..num_segs {
        let end = b.u16(p);
        let start = b.u16(q);
        p += 2;
        q += 2;
        if charcode < start {
            break;
        }
        if charcode <= end {
            let mut r = q - 2 + num_segs2;
            let mut delta = b.i16(r);
            r += num_segs2;
            let mut offset = b.u16(r) as usize;
            // Some fonts have an incorrect last segment.
            if i >= num_segs - 1 && start == 0xFFFF && end == 0xFFFF && offset != 0 && r + offset + 2 > ctx.cmap_limit {
                delta = 1;
                offset = 0;
            }
            if offset == 0xFFFF {
                continue;
            }
            if offset != 0 {
                r += offset + (charcode - start) as usize * 2;
                gindex = b.u16(r);
                if gindex != 0 {
                    gindex = ((gindex as i32 + delta) as u32) & 0xFFFF;
                    if gindex >= ctx.num_glyphs {
                        gindex = 0;
                    }
                }
            } else {
                gindex = ((charcode as i32 + delta) as u32) & 0xFFFF;
            }
            break;
        }
    }
    gindex
}

/// `tt_cmap4_char_map_binary` with `next = 0`.
fn cmap4_char_map_binary(ctx: LookupContext, cm: &CharMap, table: usize, charcode: u32) -> u32 {
    let b = Bytes(ctx.data);
    let num_segs = (b.u16(table + 6) >> 1) as usize;
    if num_segs == 0 {
        return 0;
    }
    let num_segs2 = num_segs << 1;
    let mut min = 0usize;
    let mut max = num_segs;
    let mut gindex = 0;
    loop {
        let mut mid = usize::midpoint(min, max);
        let mut p = table + 14 + mid * 2;
        let mut end = b.u16(p);
        p += 2 + num_segs2;
        let mut start = b.u16(p);

        if charcode < start {
            max = mid;
        } else if charcode > end {
            min = mid + 1;
        } else {
            p += num_segs2;
            let mut delta = b.i16(p);
            p += num_segs2;
            let mut offset = b.u16(p) as usize;

            // Some fonts have an incorrect last segment.
            if mid >= num_segs - 1 && start == 0xFFFF && end == 0xFFFF && offset != 0 && p + offset + 2 > ctx.cmap_limit
            {
                delta = 1;
                offset = 0;
            }

            // Search the first segment containing `charcode'.
            if cm.flags & TT_CMAP_FLAG_OVERLAPPING != 0 {
                // Call the current segment `max'.
                max = mid;
                if offset == 0xFFFF {
                    mid = max + 1;
                }
                // Search in segments before the current segment.
                let mut i = max;
                while i > 0 {
                    let old_p = p;
                    p = table + 14 + (i - 1) * 2;
                    let prev_end = b.u16(p);
                    if charcode > prev_end {
                        p = old_p;
                        break;
                    }
                    end = prev_end;
                    p += 2 + num_segs2;
                    start = b.u16(p);
                    p += num_segs2;
                    delta = b.i16(p);
                    p += num_segs2;
                    offset = b.u16(p) as usize;
                    if offset != 0xFFFF {
                        mid = i - 1;
                    }
                    i -= 1;
                }

                // No luck.
                if mid == max + 1 {
                    if i != max {
                        p = table + 14 + max * 2;
                        end = b.u16(p);
                        p += 2 + num_segs2;
                        start = b.u16(p);
                        p += num_segs2;
                        delta = b.i16(p);
                        p += num_segs2;
                        offset = b.u16(p) as usize;
                    }
                    mid = max;
                    // Search in segments after the current segment.
                    i = max + 1;
                    while i < num_segs {
                        p = table + 14 + i * 2;
                        let next_end = b.u16(p);
                        p += 2 + num_segs2;
                        let next_start = b.u16(p);
                        if charcode < next_start {
                            break;
                        }
                        end = next_end;
                        start = next_start;
                        p += num_segs2;
                        delta = b.i16(p);
                        p += num_segs2;
                        offset = b.u16(p) as usize;
                        if offset != 0xFFFF {
                            mid = i;
                        }
                        i += 1;
                    }
                    i = i.wrapping_sub(1);

                    // Still no luck.
                    if mid == max {
                        mid = i;
                        let _ = mid;
                        break;
                    }
                }

                // end, start, delta, and offset are for the i'th segment.
                if mid != i {
                    p = table + 14 + mid * 2;
                    end = b.u16(p);
                    p += 2 + num_segs2;
                    start = b.u16(p);
                    p += num_segs2;
                    delta = b.i16(p);
                    p += num_segs2;
                    offset = b.u16(p) as usize;
                }
                let _ = end;
            } else if offset == 0xFFFF {
                break;
            }

            if offset != 0 {
                p += offset + (charcode.wrapping_sub(start)) as usize * 2;
                gindex = b.u16(p);
                if gindex != 0 {
                    gindex = ((gindex as i32 + delta) as u32) & 0xFFFF;
                    if gindex >= ctx.num_glyphs {
                        gindex = 0;
                    }
                }
            } else {
                gindex = ((charcode as i32 + delta) as u32) & 0xFFFF;
            }
            break;
        }
        if min >= max {
            break;
        }
    }
    gindex
}

/// `tt_cmap12_char_map_binary` / `tt_cmap13_char_map_binary` with
/// `next = 0`.
fn cmap12_char_map_binary(b: Bytes, table: usize, char_code: u32, format13: bool) -> u32 {
    let num_groups = b.u32(table + 12);
    if num_groups == 0 {
        return 0;
    }
    let mut min = 0u32;
    let mut max = num_groups;
    let mut gindex = 0;
    loop {
        let mid = u32::midpoint(min, max);
        let p = table + 16 + 12 * mid as usize;
        let start = b.u32(p);
        let end = b.u32(p + 4);
        if char_code < start {
            max = mid;
        } else if char_code > end {
            min = mid + 1;
        } else {
            let start_id = b.u32(p + 8);
            if format13 {
                gindex = start_id;
            } else if start_id > 0xFFFF_FFFF - (char_code - start) {
                // Reject invalid glyph index.
                gindex = 0;
            } else {
                gindex = start_id + (char_code - start);
            }
            break;
        }
        if min >= max {
            break;
        }
    }
    gindex
}

/// `FT_Get_Char_Index`: 0 without a selected charmap, and for glyph
/// indices outside the face.
#[must_use]
pub fn get_char_index(ctx: LookupContext, charmap: Option<&CharMap>, charcode: u32) -> u32 {
    let Some(cm) = charmap else {
        return 0;
    };
    let result = char_index(ctx, cm, charcode);
    if result >= ctx.num_glyphs { 0 } else { result }
}

/// `FT_Face_GetCharVariantIndex` + `tt_cmap14_char_var_index`.
#[must_use]
pub fn get_char_variant_index(
    ctx: LookupContext,
    charmaps: &[CharMap],
    charmap: Option<usize>,
    charcode: u32,
    variant_selector: u32,
) -> u32 {
    let Some(ucmap) = charmap.map(|i| &charmaps[i]) else {
        return 0;
    };
    if ucmap.encoding != Encoding::Unicode {
        return 0;
    }
    let Some(vcmap) = find_variant_selector_charmap(charmaps).map(|i| &charmaps[i]) else {
        return 0;
    };
    let b = Bytes(ctx.data);
    let data = vcmap.data;

    // tt_cmap14_find_variant(ttcmap->data + 6, variantSelector)
    let base = data + 6;
    let num_var = b.u32(base);
    let (mut min, mut max) = (0u32, num_var);
    let mut found = None;
    while min < max {
        let mid = u32::midpoint(min, max);
        let p = base + 4 + 11 * mid as usize;
        let var_sel = b.u24(p);
        match variant_selector.cmp(&var_sel) {
            Ordering::Less => max = mid,
            Ordering::Greater => min = mid + 1,
            Ordering::Equal => {
                found = Some(p + 3);
                break;
            }
        }
    }
    let Some(p) = found else {
        return 0;
    };
    let def_off = b.u32(p) as usize;
    let nondef_off = b.u32(p + 4) as usize;

    if def_off != 0 && cmap14_char_map_def_binary(b, data + def_off, charcode) {
        // The default variant of this charcode: the GID is in the normal
        // Unicode charmap.
        return char_index(ctx, ucmap, charcode);
    }
    if nondef_off != 0 {
        return cmap14_char_map_nondef_binary(b, data + nondef_off, charcode);
    }
    0
}

/// `tt_cmap14_char_map_def_binary`.
fn cmap14_char_map_def_binary(b: Bytes, base: usize, char_code: u32) -> bool {
    let num_ranges = b.u32(base);
    let (mut min, mut max) = (0u32, num_ranges);
    let base = base + 4;
    while min < max {
        let mid = u32::midpoint(min, max);
        let p = base + 4 * mid as usize;
        let start = b.u24(p);
        let cnt = b.u8(p + 3);
        if char_code < start {
            max = mid;
        } else if char_code > start + cnt {
            min = mid + 1;
        } else {
            return true;
        }
    }
    false
}

/// `tt_cmap14_char_map_nondef_binary`.
fn cmap14_char_map_nondef_binary(b: Bytes, base: usize, char_code: u32) -> u32 {
    let num_mappings = b.u32(base);
    let (mut min, mut max) = (0u32, num_mappings);
    let base = base + 4;
    while min < max {
        let mid = u32::midpoint(min, max);
        let p = base + 5 * mid as usize;
        let uni = b.u24(p);
        match char_code.cmp(&uni) {
            Ordering::Less => max = mid,
            Ordering::Greater => min = mid + 1,
            Ordering::Equal => return b.u16(p + 3),
        }
    }
    0
}
