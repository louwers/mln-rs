// Ports src/libnu/ducet.c and include/libnu/ducet.h.

use crate::tables::ducet::{NU_DUCET_G, NU_DUCET_VALUES_C, NU_DUCET_VALUES_I};
use crate::tables::ducet_switch::{_NU_DUCET_CONTRACTIONS, nu_ducet_weight_switch};
use crate::udb::nu_udb_lookup_value;

fn nu_ducet_weights_count() -> usize {
    NU_DUCET_G.len() + _NU_DUCET_CONTRACTIONS
}

/// `nu_ducet_weight(codepoint, weight, context)`: `weight` is the C
/// `int32_t*` (null on the first call for a codepoint, else the negative
/// state of a contraction in progress, which the function overwrites with
/// the rollback count).
#[must_use]
pub fn nu_ducet_weight(codepoint: u32, weight: Option<&mut i32>) -> i32 {
    debug_assert!(nu_ducet_weights_count() < 0x7FFF_FFFF - 0x0010_FFFF);

    let switch_value = nu_ducet_weight_switch(codepoint, weight);
    // weight switch should return weight (if any) and fill value of *weight
    // with fallback (if needed). returned value of 0 is impossible result -
    // this special case is already handled above, this return value
    // indicates that switch couldn't find weight for a codepoint
    if switch_value != 0 {
        return switch_value;
    }

    // special case switch after contractions switch to let state-machine
    // figure out its state on abort
    if codepoint == 0 {
        return 0;
    }

    let mph_value = nu_udb_lookup_value(codepoint, &NU_DUCET_G, &NU_DUCET_VALUES_C, &NU_DUCET_VALUES_I);

    // ISO/IEC 14651 requests that codepoints with undefined weight should be
    // sorted before max weight in collation table. This way all codepoints
    // defined in ducet would have weight under a value of
    // nu_ducet_weights_count(), all undefined codepoints would have weight
    // under 0x10FFFF + nu_ducet_weights_count() - 1.
    if mph_value != 0 { mph_value as i32 } else { (codepoint as usize + nu_ducet_weights_count()) as i32 }
}
