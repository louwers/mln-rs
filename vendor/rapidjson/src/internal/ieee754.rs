// Ports include/rapidjson/internal/ieee754.h (RapidJSON, (C) 2015 THL A29
// Limited, a Tencent company, and Milo Yip, MIT License).

const SIGNIFICAND_SIZE: i32 = 52;
const EXPONENT_BIAS: i32 = 0x3FF;
const DENORMAL_EXPONENT: i32 = 1 - EXPONENT_BIAS;
const SIGN_MASK: u64 = 0x8000_0000_0000_0000;
const EXPONENT_MASK: u64 = 0x7FF0_0000_0000_0000;
const SIGNIFICAND_MASK: u64 = 0x000F_FFFF_FFFF_FFFF;
const HIDDEN_BIT: u64 = 0x0010_0000_0000_0000;

/// `rapidjson::internal::Double`: a double viewed as its bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Double {
    u: u64,
}

impl Double {
    #[must_use]
    pub fn from_f64(d: f64) -> Self {
        Double { u: d.to_bits() }
    }

    #[must_use]
    pub fn from_u64(u: u64) -> Self {
        Double { u }
    }

    #[must_use]
    pub fn value(&self) -> f64 {
        f64::from_bits(self.u)
    }

    #[must_use]
    pub fn uint64_value(&self) -> u64 {
        self.u
    }

    #[must_use]
    pub fn next_positive_double(&self) -> f64 {
        debug_assert!(!self.sign());
        Double::from_u64(self.u.wrapping_add(1)).value()
    }

    #[must_use]
    pub fn sign(&self) -> bool {
        (self.u & SIGN_MASK) != 0
    }

    #[must_use]
    pub fn significand(&self) -> u64 {
        self.u & SIGNIFICAND_MASK
    }

    #[must_use]
    pub fn exponent(&self) -> i32 {
        // static_cast<int>(((u_ & kExponentMask) >> kSignificandSize) - kExponentBias)
        (((self.u & EXPONENT_MASK) >> SIGNIFICAND_SIZE).wrapping_sub(EXPONENT_BIAS as u64)) as i32
    }

    #[must_use]
    pub fn is_nan(&self) -> bool {
        (self.u & EXPONENT_MASK) == EXPONENT_MASK && self.significand() != 0
    }

    #[must_use]
    pub fn is_inf(&self) -> bool {
        (self.u & EXPONENT_MASK) == EXPONENT_MASK && self.significand() == 0
    }

    #[must_use]
    pub fn is_nan_or_inf(&self) -> bool {
        (self.u & EXPONENT_MASK) == EXPONENT_MASK
    }

    #[must_use]
    pub fn is_normal(&self) -> bool {
        (self.u & EXPONENT_MASK) != 0 || self.significand() == 0
    }

    #[must_use]
    pub fn is_zero(&self) -> bool {
        (self.u & (EXPONENT_MASK | SIGNIFICAND_MASK)) == 0
    }

    #[must_use]
    pub fn integer_significand(&self) -> u64 {
        if self.is_normal() { self.significand() | HIDDEN_BIT } else { self.significand() }
    }

    #[must_use]
    pub fn integer_exponent(&self) -> i32 {
        (if self.is_normal() { self.exponent() } else { DENORMAL_EXPONENT }) - SIGNIFICAND_SIZE
    }

    #[must_use]
    pub fn to_bias(&self) -> u64 {
        if (self.u & SIGN_MASK) != 0 { (!self.u).wrapping_add(1) } else { self.u | SIGN_MASK }
    }

    #[must_use]
    pub fn effective_significand_size(order: i32) -> i32 {
        if order >= -1021 {
            53
        } else if order <= -1074 {
            0
        } else {
            order + 1074
        }
    }
}
