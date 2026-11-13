// ICU 78.3's Arabic shaping (ushape.cpp's `u_shapeArabic`) for the options
// the BiDi code passes, `U_SHAPE_LETTERS_SHAPE |
// U_SHAPE_TEXT_DIRECTION_LOGICAL` (so `U_SHAPE_LAMALEF_RESIZE`, no tashkeel
// option, no digit shaping, the old tail character, presentation forms
// converted). For those options `handle_generated_spaces` only removes the
// lam-alef placeholders and ICU's `expandCompositChar` does nothing; the
// other option paths (unshaping, tashkeel handling, near/begin/end lam-alef
// spaces, aggregation, digits) are left out.

use std::cmp::Ordering;

use super::UErrorCode;

const IRRELEVANT: u16 = 4;
const LAMTYPE: u16 = 16;
const ALEFTYPE: u16 = 32;
const LINKR: u16 = 1;
const LINKL: u16 = 2;
const APRESENT: u16 = 8;
const SHADDA: u16 = 64;
const CSHADDA: u16 = 128;
const COMBINE: u16 = SHADDA + CSHADDA;

const YEH_HAMZA_CHAR: u16 = 0x0626;
const LAMALEF_SPACE_SUB: u16 = 0xFFFF;
const LAM_CHAR: u16 = 0x0644;
const SPACE_CHAR: u16 = 0x0020;
const SHADDA06_CHAR: u16 = 0x0651;

/// `U_SHAPE_LETTERS_SHAPE | U_SHAPE_TEXT_DIRECTION_LOGICAL`.
pub const U_SHAPE_LETTERS_SHAPE: u32 = 8;
pub const U_SHAPE_TEXT_DIRECTION_LOGICAL: u32 = 0;

static IRRELEVANT_POS: [u8; 8] = [0x0, 0x2, 0x4, 0x6, 0x8, 0xA, 0xC, 0xE];

#[rustfmt::skip]
static ARA_LINK: [u16; 178] = [
  1           + 32 + 256 * 0x11,/*0x0622*/
  1           + 32 + 256 * 0x13,/*0x0623*/
  1                + 256 * 0x15,/*0x0624*/
  1           + 32 + 256 * 0x17,/*0x0625*/
  1 + 2            + 256 * 0x19,/*0x0626*/
  1           + 32 + 256 * 0x1D,/*0x0627*/
  1 + 2            + 256 * 0x1F,/*0x0628*/
  1                + 256 * 0x23,/*0x0629*/
  1 + 2            + 256 * 0x25,/*0x062A*/
  1 + 2            + 256 * 0x29,/*0x062B*/
  1 + 2            + 256 * 0x2D,/*0x062C*/
  1 + 2            + 256 * 0x31,/*0x062D*/
  1 + 2            + 256 * 0x35,/*0x062E*/
  1                + 256 * 0x39,/*0x062F*/
  1                + 256 * 0x3B,/*0x0630*/
  1                + 256 * 0x3D,/*0x0631*/
  1                + 256 * 0x3F,/*0x0632*/
  1 + 2            + 256 * 0x41,/*0x0633*/
  1 + 2            + 256 * 0x45,/*0x0634*/
  1 + 2            + 256 * 0x49,/*0x0635*/
  1 + 2            + 256 * 0x4D,/*0x0636*/
  1 + 2            + 256 * 0x51,/*0x0637*/
  1 + 2            + 256 * 0x55,/*0x0638*/
  1 + 2            + 256 * 0x59,/*0x0639*/
  1 + 2            + 256 * 0x5D,/*0x063A*/
  0, 0, 0, 0, 0,                /*0x063B-0x063F*/
  1 + 2,                        /*0x0640*/
  1 + 2            + 256 * 0x61,/*0x0641*/
  1 + 2            + 256 * 0x65,/*0x0642*/
  1 + 2            + 256 * 0x69,/*0x0643*/
  1 + 2       + 16 + 256 * 0x6D,/*0x0644*/
  1 + 2            + 256 * 0x71,/*0x0645*/
  1 + 2            + 256 * 0x75,/*0x0646*/
  1 + 2            + 256 * 0x79,/*0x0647*/
  1                + 256 * 0x7D,/*0x0648*/
  1                + 256 * 0x7F,/*0x0649*/
  1 + 2            + 256 * 0x81,/*0x064A*/
         4         + 256,       /*0x064B*/
         4 + 128   + 256,       /*0x064C*/
         4 + 128   + 256,       /*0x064D*/
         4 + 128   + 256,       /*0x064E*/
         4 + 128   + 256,       /*0x064F*/
         4 + 128   + 256,       /*0x0650*/
         4 + 64    + 256 * 3,   /*0x0651*/
         4         + 256,       /*0x0652*/
         4         + 256 * 7,   /*0x0653*/
         4         + 256 * 8,   /*0x0654*/
         4         + 256 * 8,   /*0x0655*/
         4         + 256,       /*0x0656*/
  0, 0, 0, 0, 0,                /*0x0657-0x065B*/
  1                + 256 * 0x85,/*0x065C*/
  1                + 256 * 0x87,/*0x065D*/
  1                + 256 * 0x89,/*0x065E*/
  1                + 256 * 0x8B,/*0x065F*/
  0, 0, 0, 0, 0,                /*0x0660-0x0664*/
  0, 0, 0, 0, 0,                /*0x0665-0x0669*/
  0, 0, 0, 0, 0, 0,             /*0x066A-0x066F*/
         4         + 256 * 6,   /*0x0670*/
  1        + 8,                 /*0x0671*/
  1            + 32,            /*0x0672*/
  1            + 32,            /*0x0673*/
  0,                            /*0x0674*/
  1            + 32,            /*0x0675*/
  1, 1,                         /*0x0676-0x0677*/
  1 + 2,                        /*0x0678*/
  1 + 2 + 8        + 256 * 0x16,/*0x0679*/
  1 + 2 + 8        + 256 * 0x0E,/*0x067A*/
  1 + 2 + 8        + 256 * 0x02,/*0x067B*/
  1+2, 1+2,                     /*0x67C-0x067D*/
  1+2+8+256 * 0x06, 1+2, 1+2, 1+2, 1+2, 1+2, /*0x067E-0x0683*/
  1+2, 1+2, 1+2+8+256 * 0x2A, 1+2,           /*0x0684-0x0687*/
  1     + 8        + 256 * 0x38,/*0x0688*/
  1, 1, 1,                      /*0x0689-0x068B*/
  1     + 8        + 256 * 0x34,/*0x068C*/
  1     + 8        + 256 * 0x32,/*0x068D*/
  1     + 8        + 256 * 0x36,/*0x068E*/
  1, 1,                         /*0x068F-0x0690*/
  1     + 8        + 256 * 0x3C,/*0x0691*/
  1, 1, 1, 1, 1, 1, 1+8+256 * 0x3A, 1,       /*0x0692-0x0699*/
  1+2, 1+2, 1+2, 1+2, 1+2, 1+2, /*0x069A-0x06A3*/
  1+2, 1+2, 1+2, 1+2,           /*0x069A-0x06A3*/
  1+2, 1+2, 1+2, 1+2, 1+2, 1+2+8+256 * 0x3E, /*0x06A4-0x06AD*/
  1+2, 1+2, 1+2, 1+2,           /*0x06A4-0x06AD*/
  1+2, 1+2+8+256 * 0x42, 1+2, 1+2, 1+2, 1+2, /*0x06AE-0x06B7*/
  1+2, 1+2, 1+2, 1+2,           /*0x06AE-0x06B7*/
  1+2, 1+2,                     /*0x06B8-0x06B9*/
  1     + 8        + 256 * 0x4E,/*0x06BA*/
  1 + 2 + 8        + 256 * 0x50,/*0x06BB*/
  1+2, 1+2,                     /*0x06BC-0x06BD*/
  1 + 2 + 8        + 256 * 0x5A,/*0x06BE*/
  1+2,                          /*0x06BF*/
  1     + 8        + 256 * 0x54,/*0x06C0*/
  1 + 2 + 8        + 256 * 0x56,/*0x06C1*/
  1, 1, 1,                      /*0x06C2-0x06C4*/
  1     + 8        + 256 * 0x90,/*0x06C5*/
  1     + 8        + 256 * 0x89,/*0x06C6*/
  1     + 8        + 256 * 0x87,/*0x06C7*/
  1     + 8        + 256 * 0x8B,/*0x06C8*/
  1     + 8        + 256 * 0x92,/*0x06C9*/
  1,                            /*0x06CA*/
  1     + 8        + 256 * 0x8E,/*0x06CB*/
  1 + 2 + 8        + 256 * 0xAC,/*0x06CC*/
  1,                            /*0x06CD*/
  1+2, 1+2,                     /*0x06CE-0x06CF*/
  1 + 2 + 8        + 256 * 0x94,/*0x06D0*/
  1+2,                          /*0x06D1*/
  1     + 8        + 256 * 0x5E,/*0x06D2*/
  1     + 8        + 256 * 0x60 /*0x06D3*/
];

#[rustfmt::skip]
static PRES_A_LINK: [u8; 275] = [
/***********0*****1*****2*****3*****4*****5*****6*****7*****8*****9*****A*****B*****C*****D*****E*****F*/
/*FB5*/    0,    1,    0,    0,    0,    0,    0,    1,    2,1 + 2,    0,    0,    0,    0,    0,    0,
/*FB6*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FB7*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    1,    2,1 + 2,    0,    0,
/*FB8*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    1,    0,    0,    0,    1,
/*FB9*/    2,1 + 2,    0,    1,    2,1 + 2,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FBA*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FBB*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FBC*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FBD*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FBE*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FBF*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    1,    2,1 + 2,
/*FC0*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FC1*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FC2*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FC3*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FC4*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,
/*FC5*/    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    0,    4,    4,
/*FC6*/    4,    4,    4
];

#[rustfmt::skip]
static PRES_B_LINK: [u8; 144] = [
/***********0*****1*****2*****3*****4*****5*****6*****7*****8*****9*****A*****B*****C*****D*****E*****F*/
/*FE7*/1 + 2,1 + 2,1 + 2,    0,1 + 2,    0,1 + 2,1 + 2,1 + 2,1 + 2,1 + 2,1 + 2,1 + 2,1 + 2,1 + 2,1 + 2,
/*FE8*/    0,    0,    1,    0,    1,    0,    1,    0,    1,    0,    1,    2,1 + 2,    0,    1,    0,
/*FE9*/    1,    2,1 + 2,    0,    1,    0,    1,    2,1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,
/*FEA*/1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,1 + 2,    0,    1,    0,    1,    0,    1,    0,
/*FEB*/    1,    0,    1,    2,1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,
/*FEC*/1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,
/*FED*/1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,
/*FEE*/1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,1 + 2,    0,    1,    2,1 + 2,    0,    1,    0,
/*FEF*/    1,    0,    1,    2,1 + 2,    0,    1,    0,    1,    0,    1,    0,    1,    0,    0,    0
];

#[rustfmt::skip]
static CONVERT_FB_TO_06: [u16; 176] = [
/***********0******1******2******3******4******5******6******7******8******9******A******B******C******D******E******F***/
/*FB5*/   0x671, 0x671, 0x67B, 0x67B, 0x67B, 0x67B, 0x67E, 0x67E, 0x67E, 0x67E,     0,     0,     0,     0, 0x67A, 0x67A,
/*FB6*/   0x67A, 0x67A,     0,     0,     0,     0, 0x679, 0x679, 0x679, 0x679,     0,     0,     0,     0,     0,     0,
/*FB7*/       0,     0,     0,     0,     0,     0,     0,     0,     0,     0, 0x686, 0x686, 0x686, 0x686,     0,     0,
/*FB8*/       0,     0, 0x68D, 0x68D, 0x68C, 0x68C, 0x68E, 0x68E, 0x688, 0x688, 0x698, 0x698, 0x691, 0x691, 0x6A9, 0x6A9,
/*FB9*/   0x6A9, 0x6A9, 0x6AF, 0x6AF, 0x6AF, 0x6AF,     0,     0,     0,     0,     0,     0,     0,     0, 0x6BA, 0x6BA,
/*FBA*/   0x6BB, 0x6BB, 0x6BB, 0x6BB, 0x6C0, 0x6C0, 0x6C1, 0x6C1, 0x6C1, 0x6C1, 0x6BE, 0x6BE, 0x6BE, 0x6BE, 0x6d2, 0x6D2,
/*FBB*/   0x6D3, 0x6D3,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,
/*FBC*/       0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,
/*FBD*/       0,     0,     0,     0,     0,     0,     0, 0x6C7, 0x6C7, 0x6C6, 0x6C6, 0x6C8, 0x6C8,     0, 0x6CB, 0x6CB,
/*FBE*/   0x6C5, 0x6C5, 0x6C9, 0x6C9, 0x6D0, 0x6D0, 0x6D0, 0x6D0,     0,     0,     0,     0,     0,     0,     0,     0,
/*FBF*/       0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0,     0, 0x6CC, 0x6CC, 0x6CC, 0x6CC
];

#[rustfmt::skip]
static CONVERT_FE_TO_06: [u16; 141] = [
/***********0******1******2******3******4******5******6******7******8******9******A******B******C******D******E******F***/
/*FE7*/   0x64B, 0x64B, 0x64C, 0x64C, 0x64D, 0x64D, 0x64E, 0x64E, 0x64F, 0x64F, 0x650, 0x650, 0x651, 0x651, 0x652, 0x652,
/*FE8*/   0x621, 0x622, 0x622, 0x623, 0x623, 0x624, 0x624, 0x625, 0x625, 0x626, 0x626, 0x626, 0x626, 0x627, 0x627, 0x628,
/*FE9*/   0x628, 0x628, 0x628, 0x629, 0x629, 0x62A, 0x62A, 0x62A, 0x62A, 0x62B, 0x62B, 0x62B, 0x62B, 0x62C, 0x62C, 0x62C,
/*FEA*/   0x62C, 0x62D, 0x62D, 0x62D, 0x62D, 0x62E, 0x62E, 0x62E, 0x62E, 0x62F, 0x62F, 0x630, 0x630, 0x631, 0x631, 0x632,
/*FEB*/   0x632, 0x633, 0x633, 0x633, 0x633, 0x634, 0x634, 0x634, 0x634, 0x635, 0x635, 0x635, 0x635, 0x636, 0x636, 0x636,
/*FEC*/   0x636, 0x637, 0x637, 0x637, 0x637, 0x638, 0x638, 0x638, 0x638, 0x639, 0x639, 0x639, 0x639, 0x63A, 0x63A, 0x63A,
/*FED*/   0x63A, 0x641, 0x641, 0x641, 0x641, 0x642, 0x642, 0x642, 0x642, 0x643, 0x643, 0x643, 0x643, 0x644, 0x644, 0x644,
/*FEE*/   0x644, 0x645, 0x645, 0x645, 0x645, 0x646, 0x646, 0x646, 0x646, 0x647, 0x647, 0x647, 0x647, 0x648, 0x648, 0x649,
/*FEF*/   0x649, 0x64A, 0x64A, 0x64A, 0x64A, 0x65C, 0x65C, 0x65D, 0x65D, 0x65E, 0x65E, 0x65F, 0x65F
];

#[rustfmt::skip]
static SHAPE_TABLE: [[[u8; 4]; 4]; 4] = [
  [ [0,0,0,0], [0,0,0,0], [0,1,0,3], [0,1,0,1] ],
  [ [0,0,2,2], [0,0,1,2], [0,1,1,2], [0,1,1,3] ],
  [ [0,0,0,0], [0,0,0,0], [0,1,0,3], [0,1,0,3] ],
  [ [0,0,1,2], [0,0,1,2], [0,1,1,2], [0,1,1,3] ]
];

/// Inverts the buffer, it's used in case the user specifies the buffer to be
/// `U_SHAPE_TEXT_DIRECTION_LOGICAL`.
fn invert_buffer(buffer: &mut [u16], size: i32, lowlimit: i32, highlimit: i32) {
    let mut i = lowlimit;
    let mut j = size - highlimit - 1;
    while i < j {
        buffer.swap(i as usize, j as usize);
        i += 1;
        j -= 1;
    }
}

/// Converts the Alef characters into an equivalent LamAlef location in the
/// 0x06xx Range, this is an intermediate stage in the operation of the
/// program later it'll be converted into the 0xFExx LamAlefs in the shaping
/// function.
fn change_lam_alef(ch: u16) -> u16 {
    match ch {
        0x0622 => 0x065C,
        0x0623 => 0x065D,
        0x0625 => 0x065E,
        0x0627 => 0x065F,
        _ => 0,
    }
}

/// Resolves the link between the characters as Arabic characters have four
/// forms : Isolated, Initial, Middle and Final Form.
fn get_link(ch: u16) -> u16 {
    if (0x0622..=0x06D3).contains(&ch) {
        ARA_LINK[(ch - 0x0622) as usize]
    } else if ch == 0x200D {
        3
    } else if (0x206D..=0x206F).contains(&ch) {
        4
    } else if (0xFB50..=0xFC62).contains(&ch) {
        u16::from(PRES_A_LINK[(ch - 0xFB50) as usize])
    } else if (0xFE70..=0xFEFC).contains(&ch) {
        u16::from(PRES_B_LINK[(ch - 0xFE70) as usize])
    } else {
        0
    }
}

/// Counts the number of spaces at each end of the logical buffer. `dest`
/// must have an element past `size` (ICU reads it when the buffer is all
/// spaces).
fn count_spaces(dest: &[u16], mut size: i32) -> (i32, i32) {
    let mut i = 0;
    let mut countl = 0;
    let mut countr = 0;
    while dest[i as usize] == SPACE_CHAR && countl < size {
        countl += 1;
        i += 1;
    }
    if countl < size {
        /* the entire buffer is not all space */
        while dest[(size - 1) as usize] == SPACE_CHAR {
            countr += 1;
            size -= 1;
        }
    }
    (countl, countr)
}

/// Whether `ch` is a Tashkeel character in the 06 range.
fn is_tashkeel_char(ch: u16) -> bool {
    (0x064B..=0x0652).contains(&ch)
}

/// Whether `ch` is a Tashkeel character in the FE range.
fn is_tashkeel_char_fe(ch: u16) -> bool {
    (0xFE70..=0xFE7F).contains(&ch)
}

fn is_alef_char(ch: u16) -> bool {
    ch == 0x0622 || ch == 0x0623 || ch == 0x0625 || ch == 0x0627
}

fn is_seen_family_char(ch: u16) -> bool {
    (0x633..=0x636).contains(&ch)
}

/// The destination size after merging lam+alef, for the supported options;
/// FE-range tashkeel also decrements it (ICU's condition for the resize
/// options).
fn calculate_size(source: &[u16], source_length: i32) -> i32 {
    let mut dest_size = source_length;
    // lamAlefOption = 1 (LETTERS_SHAPE with LAMALEF_RESIZE).
    for i in 0..source_length {
        let iu = i as usize;
        if (source[iu] == LAM_CHAR && i < (source_length - 1) && is_alef_char(source[iu + 1]))
            || is_tashkeel_char_fe(source[iu])
        {
            dest_size -= 1;
        }
    }
    dest_size
}

/// `u_strlen` of a buffer that ICU guarantees to hold a NUL.
fn u_strlen(s: &[u16]) -> i32 {
    s.iter().position(|&c| c == 0).expect("NUL-terminated") as i32
}

/// Removes the `LAMALEF_SPACE_SUB` placeholders (the only work for the
/// supported options); the result length is the `u_strlen` of the buffer (an
/// earlier NUL in the text shortens it).
fn handle_generated_spaces(dest: &mut [u16], source_length: i32) -> i32 {
    let mut tempbuffer = vec![0u16; (source_length + 1) as usize];

    // lamAlefOption = 1, tashkeelOption = 0
    let mut i = 0i32;
    let mut j = 0i32;
    let mut count = 0;
    while i < source_length {
        if dest[i as usize] == LAMALEF_SPACE_SUB {
            j -= 1;
            count += 1;
        } else {
            tempbuffer[j as usize] = dest[i as usize];
        }
        i += 1;
        j += 1;
    }

    while count >= 0 {
        tempbuffer[i as usize] = 0x0000;
        i -= 1;
        count -= 1;
    }

    dest[..source_length as usize].copy_from_slice(&tempbuffer[..source_length as usize]);
    // u_strlen(dest): dest is the caller's buffer, NUL-padded past sourceLength.
    u_strlen(dest)

    // (LAMALEF_NEAR / BEGIN / END / AUTO and the tashkeel options: not
    // taken for these options.)
}

/// Converts an Arabic Unicode buffer in 06xx Range into a shaped arabic
/// Unicode buffer in `FExx` Range.
fn shape_unicode(dest: &mut [u16], source_length: i32) -> i32 {
    let tashkeel_flag = 1;
    let mut lamalef_found = false;
    let mut prev_link: u16 = 0;
    let mut last_link: u16 = 0;
    let mut next_link: u16 = 0;

    /* Converts the input buffer from FExx Range into 06xx Range to make
    sure that all characters are in the 06xx range even the lamalef is
    converted to the special region in the 06xx range */
    // (U_SHAPE_PRESERVE_PRESENTATION_NOOP)
    for unit in &mut dest[..source_length as usize] {
        let input_char = *unit;
        if (0xFB50..=0xFBFF).contains(&input_char) {
            let c = CONVERT_FB_TO_06[(input_char - 0xFB50) as usize];
            if c != 0 {
                *unit = c;
            }
        } else if (0xFE70..=0xFEFC).contains(&input_char) {
            *unit = CONVERT_FE_TO_06[(input_char - 0xFE70) as usize];
        } else {
            *unit = input_char;
        }
    }

    /* sets the index to the end of the buffer, together with the step point to -1 */
    let mut i = source_length - 1;
    let iend = -1;
    let step = -1;

    /* This function resolves the link between the characters. Arabic
    characters have four forms: Isolated Form, Initial Form, Middle Form and
    Final Form */
    let mut curr_link = get_link(dest[i as usize]);

    let mut last_pos = i;
    let mut nx: i32 = -2;
    let mut nw: i32;

    while i != iend {
        /* If high byte of currLink > 0 then more than one shape */
        if (curr_link & 0xFF00) > 0 || (get_link(dest[i as usize]) & IRRELEVANT) != 0 {
            nw = i + step;
            while nx < 0 {
                /* we need to know about next char */
                if nw == iend {
                    next_link = 0;
                    nx = 3000;
                } else {
                    next_link = get_link(dest[nw as usize]);
                    if (next_link & IRRELEVANT) == 0 {
                        nx = nw;
                    } else {
                        nw += step;
                    }
                }
            }

            if (curr_link & ALEFTYPE) > 0 && (last_link & LAMTYPE) > 0 {
                lamalef_found = true;
                let w_lamalef = change_lam_alef(dest[i as usize]); /*get from 0x065C-0x065f */
                if w_lamalef != 0 {
                    dest[i as usize] = LAMALEF_SPACE_SUB; /* The default case is to drop the Alef and replace */
                    dest[last_pos as usize] = w_lamalef; /* it by LAMALEF_SPACE_SUB which is the last character in the  */
                    i = last_pos; /* unicode private use area, this is done to make   */
                } /* sure that removeLamAlefSpaces() handles only the */
                last_link = prev_link; /* spaces generated during lamalef generation.      */
                curr_link = get_link(w_lamalef); /* LAMALEF_SPACE_SUB is added here and is replaced by spaces   */
            } /* in removeLamAlefSpaces()                         */

            // (seenfamFound / yehhamzaFound only matter to
            // expandCompositChar, which does nothing for these options.)
            let _ = (is_seen_family_char(dest[i as usize]), dest[i as usize] == YEH_HAMZA_CHAR);

            /* get the proper shape according to link ability of neighbors
            and of character; depends on the order of the shapes (isolated,
            initial, middle, final) in the compatibility area */
            let mut shape = u32::from(
                SHAPE_TABLE[(next_link & (LINKR + LINKL)) as usize][(last_link & (LINKR + LINKL)) as usize]
                    [(curr_link & (LINKR + LINKL)) as usize],
            );

            if (curr_link & (LINKR + LINKL)) == 1 {
                shape &= 1;
            } else if is_tashkeel_char(dest[i as usize]) {
                if (last_link & LINKL) != 0
                    && (next_link & LINKR) != 0
                    && tashkeel_flag == 1
                    && dest[i as usize] != 0x064C
                    && dest[i as usize] != 0x064D
                {
                    shape = 1;
                    if (next_link & ALEFTYPE) == ALEFTYPE && (last_link & LAMTYPE) == LAMTYPE {
                        shape = 0;
                    }
                } else {
                    // (tashkeelFlag == 2: not these options)
                    shape = 0;
                }
            }
            if (dest[i as usize] ^ 0x0600) < 0x100 {
                if is_tashkeel_char(dest[i as usize]) {
                    // (tashkeelFlag == 1: the tashkeel is kept)
                    dest[i as usize] =
                        0xFE70 + u16::from(IRRELEVANT_POS[(dest[i as usize] - 0x064B) as usize]) + shape as u16;
                } else if (curr_link & APRESENT) > 0 {
                    dest[i as usize] = (0xFB50u32 + u32::from(curr_link >> 8) + shape) as u16;
                } else if (curr_link >> 8) > 0 && (curr_link & IRRELEVANT) == 0 {
                    dest[i as usize] = (0xFE70u32 + u32::from(curr_link >> 8) + shape) as u16;
                }
            }
        }

        /* move one notch forward */
        if (curr_link & IRRELEVANT) == 0 {
            prev_link = last_link;
            last_link = curr_link;
            last_pos = i;
        }

        i += step;
        if i == nx {
            curr_link = next_link;
            nx = -2;
        } else if i != iend {
            curr_link = get_link(dest[i as usize]);
        }
    }
    let mut dest_size = source_length;
    if lamalef_found {
        dest_size = handle_generated_spaces(dest, source_length);
    }

    // (seenfamFound / yehhamzaFound: expandCompositChar in SHAPE_MODE with
    // these options changes nothing.)
    let _ = (SHADDA06_CHAR, COMBINE);
    dest_size
}

/// Shapes `source` with `U_SHAPE_LETTERS_SHAPE` and
/// `U_SHAPE_TEXT_DIRECTION_LOGICAL` into a destination of `dest_capacity`
/// code units. Returns the code units written, the shaped length (which may
/// exceed `dest_capacity`), and the error code.
#[must_use]
pub fn u_shape_arabic(source: &[u16], dest_capacity: i32) -> (Vec<u16>, i32, UErrorCode) {
    let mut error = UErrorCode::U_ZERO_ERROR;
    let source_length = source.len() as i32;
    if source_length <= 0 {
        let n = terminate(dest_capacity, 0, &mut error);
        return (Vec::new(), n, error);
    }

    // (U_SHAPE_TAIL_TYPE_MASK: the old tail char; not used by these options.)

    // (options&U_SHAPE_LETTERS_MASK) == U_SHAPE_LETTERS_SHAPE
    // (no U_SHAPE_AGGREGATE_TASHKEEL)

    /* calculate destination size */
    // (LAMALEF_RESIZE)
    let mut output_size = calculate_size(source, source_length);

    if output_size > dest_capacity {
        error = UErrorCode::U_BUFFER_OVERFLOW_ERROR;
        return (Vec::new(), output_size, error);
    }

    /* need a temporary buffer of size max(outputSize, sourceLength) because
    at first we copy source->temp */
    if source_length > output_size {
        output_size = source_length;
    }

    /* Start of Arabic letter shaping part */
    // char16_t buffer[300] or a malloc'ed one; NUL-padded past sourceLength
    // (one extra element: countSpaces may read one past an all-space
    // buffer, and handleGeneratedSpaces' u_strlen needs a NUL).
    let output_size = output_size.max(300);
    let mut tempbuffer = vec![0u16; output_size as usize + 1];
    tempbuffer[..source_length as usize].copy_from_slice(source);

    // U_SHAPE_TEXT_DIRECTION_LOGICAL
    let (l, r) = count_spaces(&tempbuffer, source_length);
    invert_buffer(&mut tempbuffer, source_length, l, r);

    // U_SHAPE_LETTERS_SHAPE, no tashkeel option: tashkeel flag 1.
    let dest_length = shape_unicode(&mut tempbuffer, source_length);

    // U_SHAPE_TEXT_DIRECTION_LOGICAL
    let (l, r) = count_spaces(&tempbuffer, dest_length);
    invert_buffer(&mut tempbuffer, dest_length, l, r);
    let written = tempbuffer[..dest_length.min(dest_capacity).max(0) as usize].to_vec();

    if dest_length > dest_capacity {
        error = UErrorCode::U_BUFFER_OVERFLOW_ERROR;
        return (written, dest_length, error);
    }

    /* End of Arabic letter shaping part */

    // (no digit shaping)

    let n = terminate(dest_capacity, dest_length, &mut error);
    (written, n, error)
}

/// Sets the error code for a result of `length` code units in
/// `dest_capacity`, as ICU's `u_terminateUChars` does; returns `length`.
fn terminate(dest_capacity: i32, length: i32, error: &mut UErrorCode) -> i32 {
    match length.cmp(&dest_capacity) {
        Ordering::Less => {
            if *error == UErrorCode::U_STRING_NOT_TERMINATED_WARNING {
                *error = UErrorCode::U_ZERO_ERROR;
            }
        }
        Ordering::Equal => *error = UErrorCode::U_STRING_NOT_TERMINATED_WARNING,
        Ordering::Greater => *error = UErrorCode::U_BUFFER_OVERFLOW_ERROR,
    }
    length
}
