// Ports src/mln/text/glyph_pbf.hpp and src/mln/text/glyph_pbf.cpp.

use mln_util::image::AlphaImage;
use mln_util::size::Size;
use protozero::{Exception, PbfReader};

use crate::glyph::{Glyph, GlyphID, GlyphRange};

/// `parseGlyphPBF`: the valid glyphs of a glyph PBF within `glyph_range`.
/// Errors are protozero's exceptions (malformed varints, truncated data),
/// which the C++ lets propagate.
pub fn parse_glyph_pbf(glyph_range: &GlyphRange, data: &[u8]) -> Result<Vec<Glyph>, Exception> {
    let mut result = Vec::with_capacity(256);

    let mut glyphs_pbf = PbfReader::new(data);

    while glyphs_pbf.next_tag(1)? {
        let mut fontstack_pbf = glyphs_pbf.get_message()?;
        while fontstack_pbf.next_tag(3)? {
            let mut glyph_pbf = fontstack_pbf.get_message()?;

            let mut glyph = Glyph::default();
            let mut glyph_data: &[u8] = &[];

            let mut has_id = false;
            let mut has_width = false;
            let mut has_height = false;
            let mut has_left = false;
            let mut has_top = false;
            let mut has_advance = false;

            while glyph_pbf.next_field()? {
                match glyph_pbf.tag() {
                    1 => {
                        // id: `GlyphID(uint32_t)` keeps the low 16 bits.
                        glyph.id = GlyphID::new(glyph_pbf.get_uint32()?);
                        has_id = true;
                    }
                    2 => {
                        // bitmap
                        glyph_data = glyph_pbf.get_view()?;
                    }
                    3 => {
                        glyph.metrics.width = glyph_pbf.get_uint32()?;
                        has_width = true;
                    }
                    4 => {
                        glyph.metrics.height = glyph_pbf.get_uint32()?;
                        has_height = true;
                    }
                    5 => {
                        glyph.metrics.left = glyph_pbf.get_sint32()? as f32;
                        has_left = true;
                    }
                    6 => {
                        glyph.metrics.top = glyph_pbf.get_sint32()? as f32;
                        has_top = true;
                    }
                    7 => {
                        glyph.metrics.advance = glyph_pbf.get_uint32()?;
                        has_advance = true;
                    }
                    _ => glyph_pbf.skip()?,
                }
            }

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
                continue;
            }

            // If the area of width/height is non-zero, we need to adjust the
            // expected size with the implicit border size, otherwise we
            // expect there to be no bitmap at all.
            if m.width != 0 && m.height != 0 {
                let border = u32::from(Glyph::BORDER_SIZE);
                let size = Size::new(m.width + 2 * border, m.height + 2 * border);
                if size.area() as usize != glyph_data.len() {
                    continue;
                }
                glyph.bitmap = AlphaImage::from_slice(size, glyph_data).expect("size checked");
            }

            result.push(glyph);
        }
    }

    Ok(result)
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
