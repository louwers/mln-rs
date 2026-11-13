// Font faces from WOFF and WOFF2 files. The fixtures are one font in three
// formats: the render suite's Noto Sans Khmer, subset to the glyphs of
// "ខ្មែរ" with its layout tables, saved as TrueType, WOFF and WOFF2 by
// fontTools (`fonttools subset --text=ខ្មែរ --layout-features='*'
// --name-IDs='*' [--flavor=woff|woff2]`).

use std::sync::Arc;

use crate::freetype::FreeTypeFace;
use crate::glyph::{FontData, GlyphID, GlyphIDType};
use crate::woff::{self, WoffError};
use crate::{FreeTypeLibrary, HBShapeAdjust, HBShaper};

const TTF: &[u8] = include_bytes!("../../tests/fixtures/khmer.ttf");
const WOFF: &[u8] = include_bytes!("../../tests/fixtures/khmer.woff");
const WOFF2: &[u8] = include_bytes!("../../tests/fixtures/khmer.woff2");

/// "ខ្មែរ": a subscript consonant and a split vowel, which the font's GSUB
/// and GPOS rearrange and position.
const KHMER: &str = "ខ្មែរ";

fn shape(font: &[u8]) -> (Vec<GlyphID>, Vec<HBShapeAdjust>, Vec<crate::glyph::Glyph>) {
    let type_ = GlyphIDType(1);
    let shaper = HBShaper::new(type_, font.to_vec(), &FreeTypeLibrary::new());
    assert!(shaper.valid());
    let text: Vec<u16> = KHMER.encode_utf16().collect();
    let (mut ids, mut adjusts) = (Vec::new(), Vec::new());
    shaper.create_complex_glyph_ids(&text, false, &mut ids, &mut adjusts);
    let glyphs = ids.iter().map(|id| shaper.rasterize_glyph(id)).collect();
    (ids, adjusts, glyphs)
}

#[test]
fn woff_and_woff2_shape_and_rasterize_as_the_ttf() {
    let ttf = shape(TTF);
    // Shaping reorders and substitutes: not one glyph per code point.
    assert_ne!(ttf.0.len(), KHMER.chars().count());
    assert!(ttf.2.iter().any(|glyph| glyph.bitmap.size.width > 0));
    assert_eq!(shape(WOFF), ttf);
    assert_eq!(shape(WOFF2), ttf);
}

/// The tables of an SFNT font, by tag.
fn tables(sfnt: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let u16_at = |at: usize| usize::from(u16::from_be_bytes([sfnt[at], sfnt[at + 1]]));
    let u32_at = |at: usize| u32::from_be_bytes(sfnt[at..at + 4].try_into().unwrap()) as usize;
    (0..u16_at(4))
        .map(|i| {
            let entry = 12 + i * 16;
            let (offset, length) = (u32_at(entry + 8), u32_at(entry + 12));
            (sfnt[entry..entry + 4].try_into().unwrap(), &sfnt[offset..offset + length])
        })
        .collect()
}

#[test]
fn woff_decodes_to_the_ttf_tables() {
    let sfnt = woff::decode_woff(WOFF).unwrap();
    assert_eq!(sfnt[..4], TTF[..4]);
    assert_eq!(tables(&sfnt), tables(TTF));
    assert_eq!(sfnt.len() % 4, 0);
}

#[test]
fn woff2_decodes_to_a_font() {
    let sfnt = woff::decode_woff2(WOFF2).unwrap();
    let face = FreeTypeFace::from_memory(sfnt, &FreeTypeLibrary::new());
    assert!(face.is_valid());
    assert_eq!(
        face.get_char_index(u32::from('ខ')),
        FreeTypeFace::from_memory(TTF.to_vec(), &FreeTypeLibrary::new()).get_char_index(u32::from('ខ'))
    );
}

#[test]
fn to_sfnt_keeps_other_data_uncopied() {
    let data = FontData::from(Arc::new(TTF.to_vec()));
    let sfnt = woff::to_sfnt(data.clone()).unwrap();
    assert!(std::ptr::eq(sfnt.as_bytes(), data.as_bytes()));
    let garbage = FontData::from(b"not a font".to_vec());
    assert_eq!(woff::to_sfnt(garbage.clone()).unwrap(), garbage);
    assert!(woff::is_woff(WOFF) && woff::is_woff(WOFF2) && !woff::is_woff(TTF));
}

#[test]
fn freetype_face_opens_woff_and_woff2() {
    for font in [WOFF, WOFF2] {
        let face = FreeTypeFace::from_memory(font.to_vec(), &FreeTypeLibrary::new());
        assert!(face.is_valid());
        assert!(!woff::is_woff(face.data().as_bytes()));
    }
}

fn set_u32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_be_bytes());
}

#[test]
fn malformed_woff_is_an_error() {
    let mut wrong_length = WOFF.to_vec();
    wrong_length.push(0);
    assert_eq!(woff::decode_woff(&wrong_length), Err(WoffError::InvalidWoff));

    // The first table entry's original length, beyond what its data can
    // inflate to, with the font's size to match.
    let mut huge = WOFF.to_vec();
    set_u32(&mut huge, 44 + 12, 0x7FFF_FFF0);
    set_u32(&mut huge, 16, 0xFFFF_FFFC);
    assert_eq!(woff::decode_woff(&huge), Err(WoffError::InvalidWoff));

    // A compressed table's zlib stream, corrupted.
    let mut corrupt = WOFF.to_vec();
    let directory = 44;
    let compressed = (0..usize::from(u16::from_be_bytes([WOFF[12], WOFF[13]])))
        .map(|i| directory + i * 20)
        .find(|&entry| WOFF[entry + 8..entry + 12] != WOFF[entry + 12..entry + 16])
        .unwrap();
    let offset = u32::from_be_bytes(WOFF[compressed + 4..compressed + 8].try_into().unwrap()) as usize;
    corrupt[offset] ^= 0xFF;
    assert_eq!(woff::decode_woff(&corrupt), Err(WoffError::InvalidWoff));

    // Every other table entry pointing at that table's zlib stream.
    let mut repeated = WOFF.to_vec();
    for entry in (0..usize::from(u16::from_be_bytes([WOFF[12], WOFF[13]]))).map(|i| directory + i * 20) {
        repeated[entry + 4..entry + 16].copy_from_slice(&WOFF[compressed + 4..compressed + 16]);
    }
    assert_eq!(woff::decode_woff(&repeated), Err(WoffError::InvalidWoff));

    assert_eq!(woff::decode_woff(TTF), Err(WoffError::InvalidWoff));
    assert_eq!(woff::decode_woff2(b"wOF2"), Err(WoffError::InvalidWoff2));
    assert_eq!(woff::to_sfnt(FontData::from(b"wOFF".to_vec())), Err(WoffError::InvalidWoff));
    assert!(!FreeTypeFace::from_memory(corrupt, &FreeTypeLibrary::new()).is_valid());
}

/// A decoder from a font file to its sfnt.
type Decoder = fn(&[u8]) -> Result<Vec<u8>, WoffError>;

/// Every truncation of the fixtures fails, and every one-byte change
/// decodes or fails, without a panic.
#[test]
fn damaged_files_never_panic() {
    let decoders: [(&[u8], Decoder); 2] = [(WOFF, woff::decode_woff), (WOFF2, woff::decode_woff2)];
    for (font, decode) in decoders {
        for len in 0..font.len() {
            assert!(decode(&font[..len]).is_err(), "truncated to {len}");
        }
        for at in 0..font.len() {
            for flip in [0x01, 0x80, 0xFF] {
                let mut damaged = font.to_vec();
                damaged[at] ^= flip;
                if let Ok(sfnt) = woff::to_sfnt(FontData::from(damaged)) {
                    let _ = FreeTypeFace::from_memory(sfnt, &FreeTypeLibrary::new());
                }
            }
        }
    }
}
