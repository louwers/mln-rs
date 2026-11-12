// Ports src/mln/text/glyph_pbf.hpp and src/mln/text/glyph_pbf.cpp, reading
// the protobuf with buffa's wire format functions where the C++ uses
// protozero.

use buffa::DecodeError;
use buffa::encoding::{Tag, WireType, skip_field};
use buffa::types::{borrow_bytes, decode_sint32, decode_uint32};
use mln_util::image::AlphaImage;
use mln_util::size::Size;

use crate::glyph::{Glyph, GlyphID, GlyphRange};

/// `parseGlyphPBF`: the valid glyphs of a glyph PBF within `glyph_range`.
/// Errors: a malformed protobuf (varints, truncated data), which the C++
/// lets propagate.
pub fn parse_glyph_pbf(glyph_range: &GlyphRange, data: &[u8]) -> Result<Vec<Glyph>, DecodeError> {
    let mut result = Vec::with_capacity(256);
    // glyphs { repeated fontstack stacks = 1; }
    for_each_field(data, |tag, buf| {
        if tag.field_number() != 1 || tag.wire_type() != WireType::LengthDelimited {
            return Ok(false);
        }
        // fontstack { repeated glyph glyphs = 3; }
        for_each_field(borrow_bytes(buf)?, |tag, buf| {
            if tag.field_number() != 3 || tag.wire_type() != WireType::LengthDelimited {
                return Ok(false);
            }
            result.extend(parse_glyph(*glyph_range, borrow_bytes(buf)?)?);
            Ok(true)
        })?;
        Ok(true)
    })?;
    Ok(result)
}

/// Reads the fields of the message in `data`: `read` reads the value of
/// the field `tag` from the buffer, or returns `false` to skip it.
fn for_each_field<'a>(
    mut data: &'a [u8],
    mut read: impl FnMut(Tag, &mut &'a [u8]) -> Result<bool, DecodeError>,
) -> Result<(), DecodeError> {
    while !data.is_empty() {
        let tag = Tag::decode(&mut data)?;
        if !read(tag, &mut data)? {
            skip_field(tag, &mut data)?;
        }
    }
    Ok(())
}

/// One `glyph` message: `None` for a glyph to discard.
fn parse_glyph(glyph_range: GlyphRange, data: &[u8]) -> Result<Option<Glyph>, DecodeError> {
    let mut glyph = Glyph::default();
    let mut glyph_data: &[u8] = &[];

    let mut has_id = false;
    let mut has_width = false;
    let mut has_height = false;
    let mut has_left = false;
    let mut has_top = false;
    let mut has_advance = false;

    for_each_field(data, |tag, buf| {
        match (tag.field_number(), tag.wire_type()) {
            (1, WireType::Varint) => {
                // id: `GlyphID(uint32_t)` keeps the low 16 bits.
                glyph.id = GlyphID::new(decode_uint32(buf)?);
                has_id = true;
            }
            (2, WireType::LengthDelimited) => {
                // bitmap
                glyph_data = borrow_bytes(buf)?;
            }
            (3, WireType::Varint) => {
                glyph.metrics.width = decode_uint32(buf)?;
                has_width = true;
            }
            (4, WireType::Varint) => {
                glyph.metrics.height = decode_uint32(buf)?;
                has_height = true;
            }
            (5, WireType::Varint) => {
                glyph.metrics.left = decode_sint32(buf)? as f32;
                has_left = true;
            }
            (6, WireType::Varint) => {
                glyph.metrics.top = decode_sint32(buf)? as f32;
                has_top = true;
            }
            (7, WireType::Varint) => {
                glyph.metrics.advance = decode_uint32(buf)?;
                has_advance = true;
            }
            _ => return Ok(false),
        }
        Ok(true)
    })?;

    // Only treat this glyph as a correct glyph if it has all required
    // fields. It also needs to satisfy a few metrics conditions that
    // ensure that the glyph isn't bogus. All other glyphs are
    // malformed. We're also discarding all glyphs that are outside
    // the expected glyph range (compared as `GlyphID::hash`).
    let m = &glyph.metrics;
    if !has_id
        || !has_width
        || !has_height
        || !has_left
        || !has_top
        || !has_advance
        || m.width >= 256
        || m.height >= 256
        || m.left < -128.0
        || m.left >= 128.0
        || m.top < -128.0
        || m.top >= 128.0
        || m.advance >= 256
        || glyph.id.hash() < u32::from(glyph_range.first)
        || glyph.id.hash() > u32::from(glyph_range.second)
    {
        return Ok(None);
    }

    // If the area of width/height is non-zero, we need to adjust the
    // expected size with the implicit border size, otherwise we
    // expect there to be no bitmap at all.
    if m.width != 0 && m.height != 0 {
        let border = u32::from(Glyph::BORDER_SIZE);
        let size = Size::new(m.width + 2 * border, m.height + 2 * border);
        if size.area() as usize != glyph_data.len() {
            return Ok(None);
        }
        glyph.bitmap = AlphaImage::from_slice(size, glyph_data).expect("size checked");
    }

    Ok(Some(glyph))
}

/// Ports `test/text/glyph_pbf.test.cpp`.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::glyph::GlyphIDType;

    fn fixture(path: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(path);
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    // gtest: GlyphPBF.Parsing
    #[test]
    fn parsing() {
        // The fake glyphs contain a number of invalid glyphs, which should
        // be skipped by the parser.
        let sdfs = parse_glyph_pbf(
            &GlyphRange::new(0, 255, GlyphIDType::FontPBF),
            &fixture("test/fixtures/resources/fake_glyphs-0-255.pbf"),
        )
        .unwrap();
        assert_eq!(sdfs.len(), 1);

        let sdf = &sdfs[0];
        assert_eq!(69, sdf.id.code);
        let mut expected = AlphaImage::new(Size::new(7, 7));
        expected.fill(b'x');
        assert_eq!(expected, sdf.bitmap);
        assert_eq!(1, sdf.metrics.width);
        assert_eq!(1, sdf.metrics.height);
        assert_eq!(20.0, sdf.metrics.left);
        assert_eq!(2.0, sdf.metrics.top);
        assert_eq!(8, sdf.metrics.advance);
    }
}
