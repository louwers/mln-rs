// Ports src/libnu/strcoll.c (`_compound_weight`, `_nu_strcoll`,
// `nu_strcoll`, `nu_strcasecoll`) and the compound readers of
// include/libnu/strcoll_internal.h, for the configuration of the vendored
// copy: NU_WITH_Z_COLLATION (0-terminated strings, NU_UNLIMITED limits),
// contractions enabled (NU_DISABLE_CONTRACTIONS is not defined), and
// NU_FOLDING_FUNCTION = nu_toupper (NU_WITH_TOFOLD is not defined). The
// case-insensitive comparison folds with `char::to_uppercase`, Unicode's full
// uppercase mapping, in place of `nu_toupper`.
//
// Strings are 0-terminated C strings: reading at an embedded NUL or at the
// end of the slice yields codepoint 0. Every `p < limit` test of the C code
// is true for NU_UNLIMITED and is left out (commented where it was).

use core::char::ToUppercase;

use crate::ducet::nu_ducet_weight;
use crate::utf8::nu_utf8_read;

/// A compound tail: the rest of a codepoint's uppercase mapping (C: a
/// `const char*` into a casemapping result, or null).
type Tail = Option<ToUppercase>;

/// `nu_compound_read_t`: `nu_default_compound_read` or
/// `nu_nocase_compound_read`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompoundRead {
    Default,
    Nocase,
}

fn compound_read(com: CompoundRead, s: &[u8], encoded: usize, unicode: &mut u32, tail: &mut Tail) -> usize {
    match com {
        CompoundRead::Default => {
            // `encoded_read(encoded, unicode)`
            let (np, u) = nu_utf8_read(s, encoded);
            *unicode = u;
            np
        }
        CompoundRead::Nocase => {
            // re-entry with tail != 0
            if let Some(t) = tail {
                *unicode = t.next().map_or(0, u32::from);

                if *unicode != 0 {
                    return encoded;
                }

                *tail = None; // fall thru
            }

            // `if (encoded >= encoded_limit)`: never for NU_UNLIMITED

            let (p, u) = nu_utf8_read(s, encoded);
            *unicode = u;

            if *unicode == 0 {
                return p;
            }

            // A codepoint that is not a char (from invalid UTF-8) has no
            // mapping.
            if let Some(c) = char::from_u32(*unicode) {
                let mut map = c.to_uppercase();
                *unicode = map.next().map_or(0, u32::from);
                *tail = Some(map);
            }

            p
        }
    }
}

/// `_compound_weight`.
fn compound_weight(mut w: i32, encoded: &mut usize, s: &[u8], com: CompoundRead, tail: &mut Tail) -> i32 {
    let mut tailp = tail.clone();

    let mut p = *encoded;
    let mut new_w: i32;
    // one codepoint was consumed at the top of the stack (_nu_strcoll)
    let mut consumed: i32 = 1;

    // `while (p < limit)`: always for NU_UNLIMITED
    loop {
        let mut u = 0;

        let mut np = compound_read(com, s, p, &mut u, &mut tailp);
        new_w = nu_ducet_weight(u, Some(&mut w));

        // after this point, w might hold rollback value and new_w holds
        // actual weight

        consumed += 1;

        if new_w >= 0 {
            // if w == 0 or w == 1, then *p or *np is already pointing to
            // needed place, otherwise re-read encoded in the forward
            // direction preserving correctness of tail pointer
            if w != 0 && w != 1 {
                debug_assert!(consumed + w > 1);

                np = *encoded;
                tailp = tail.clone();

                for _ in 0..consumed - w {
                    // C passes a null `unicode` here.
                    let mut ignored = 0;
                    np = compound_read(com, s, np, &mut ignored, &mut tailp);
                }

                w = 0;
            }

            *encoded = if w == 0 { np } else { p };
            *tail = tailp;

            break;
        }

        p = np;
        w = new_w;
    }

    // `if (new_w < 0) new_w = weight(0, &w, context);`: only reachable when
    // the limit ends the loop, which NU_UNLIMITED never does.

    debug_assert!(new_w >= 0);

    new_w
}

/// `_nu_strcoll` with `NU_UNLIMITED` limits and no collated-length outputs.
fn collate(lhs: &[u8], rhs: &[u8], com1: CompoundRead, com2: CompoundRead) -> i32 {
    let mut cmp = 0;

    let (mut lp, mut rp) = (0usize, 0usize);
    let (mut ltailp, mut rtailp): (Tail, Tail) = (None, None);

    let (mut u1, mut u2) = (0u32, 0u32);

    // The loop condition `(lp < lhs_limit && rp < rhs_limit) || ...` is
    // always true for NU_UNLIMITED.
    loop {
        lp = compound_read(com1, lhs, lp, &mut u1, &mut ltailp);
        rp = compound_read(com2, rhs, rp, &mut u2, &mut rtailp);

        let mut w1 = nu_ducet_weight(u1, None);
        let mut w2 = nu_ducet_weight(u2, None);

        if w1 < 0 {
            w1 = compound_weight(w1, &mut lp, lhs, com1, &mut ltailp);
        }

        if w2 < 0 {
            w2 = compound_weight(w2, &mut rp, rhs, com2, &mut rtailp);
        }

        debug_assert!(w1 >= 0);
        debug_assert!(w2 >= 0);

        if w1 < w2 {
            cmp = -1;
            break;
        } else if w1 > w2 {
            cmp = 1;
            break;
        }

        if u1 == 0 || u2 == 0 {
            break;
        }
    }

    // The limits fix-up of cmp (`rp < rhs_limit && lp >= lhs_limit`, ...) is
    // never taken for NU_UNLIMITED.

    cmp
}

/// `nu_strcoll(s1, s2, nu_utf8_read, nu_utf8_read)` on 0-terminated UTF-8.
#[must_use]
pub fn nu_strcoll(s1: &[u8], s2: &[u8]) -> i32 {
    collate(s1, s2, CompoundRead::Default, CompoundRead::Default)
}

/// `nu_strcasecoll(s1, s2, nu_utf8_read, nu_utf8_read)` on 0-terminated
/// UTF-8.
#[must_use]
pub fn nu_strcasecoll(s1: &[u8], s2: &[u8]) -> i32 {
    collate(s1, s2, CompoundRead::Nocase, CompoundRead::Nocase)
}
