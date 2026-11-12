// Ports include/rapidjson/internal/diyfp.h (RapidJSON, (C) 2015 THL A29
// Limited, a Tencent company, and Milo Yip, MIT License): the Grisu2
// "do-it-yourself floating point" of Loitsch, "Printing floating-point
// numbers quickly and accurately with integers" (PLDI 2010).
//
// All `uint64_t` arithmetic wraps, as in C++.

pub const DIY_SIGNIFICAND_SIZE: i32 = 64;
pub const DP_SIGNIFICAND_SIZE: i32 = 52;
pub const DP_EXPONENT_BIAS: i32 = 0x3FF + DP_SIGNIFICAND_SIZE;
pub const DP_MAX_EXPONENT: i32 = 0x7FF - DP_EXPONENT_BIAS;
pub const DP_MIN_EXPONENT: i32 = -DP_EXPONENT_BIAS;
pub const DP_DENORMAL_EXPONENT: i32 = -DP_EXPONENT_BIAS + 1;
pub const DP_EXPONENT_MASK: u64 = 0x7FF0_0000_0000_0000;
pub const DP_SIGNIFICAND_MASK: u64 = 0x000F_FFFF_FFFF_FFFF;
pub const DP_HIDDEN_BIT: u64 = 0x0010_0000_0000_0000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiyFp {
    pub f: u64,
    pub e: i32,
}

impl DiyFp {
    #[must_use]
    pub const fn new(fp: u64, exp: i32) -> Self {
        DiyFp { f: fp, e: exp }
    }

    #[must_use]
    pub fn from_f64(d: f64) -> Self {
        let u64_ = d.to_bits();
        let biased_e = ((u64_ & DP_EXPONENT_MASK) >> DP_SIGNIFICAND_SIZE) as i32;
        let significand = u64_ & DP_SIGNIFICAND_MASK;
        if biased_e != 0 {
            DiyFp { f: significand.wrapping_add(DP_HIDDEN_BIT), e: biased_e - DP_EXPONENT_BIAS }
        } else {
            DiyFp { f: significand, e: DP_MIN_EXPONENT + 1 }
        }
    }

    /// `operator-`
    #[must_use]
    pub fn sub(&self, rhs: &DiyFp) -> DiyFp {
        DiyFp::new(self.f.wrapping_sub(rhs.f), self.e)
    }

    /// `operator*`: the high 64 bits of the 128-bit product, rounded up when
    /// bit 63 of the low half is set (the `unsigned __int128` branch).
    #[must_use]
    pub fn mul(&self, rhs: &DiyFp) -> DiyFp {
        let p = u128::from(self.f) * u128::from(rhs.f);
        let mut h = (p >> 64) as u64;
        let l = p as u64;
        if l & (1u64 << 63) != 0 {
            // rounding
            h = h.wrapping_add(1);
        }
        DiyFp::new(h, self.e + rhs.e + 64)
    }

    #[must_use]
    pub fn normalize(&self) -> DiyFp {
        debug_assert!(self.f != 0); // clzll's precondition
        let s = self.f.leading_zeros() as i32;
        DiyFp::new(self.f << s, self.e - s)
    }

    #[must_use]
    pub fn normalize_boundary(&self) -> DiyFp {
        let mut res = *self;
        while res.f & (DP_HIDDEN_BIT << 1) == 0 {
            res.f <<= 1;
            res.e -= 1;
        }
        res.f <<= DIY_SIGNIFICAND_SIZE - DP_SIGNIFICAND_SIZE - 2;
        res.e -= DIY_SIGNIFICAND_SIZE - DP_SIGNIFICAND_SIZE - 2;
        res
    }

    /// Returns `(minus, plus)`.
    #[must_use]
    pub fn normalized_boundaries(&self) -> (DiyFp, DiyFp) {
        let pl = DiyFp::new((self.f << 1).wrapping_add(1), self.e - 1).normalize_boundary();
        let mut mi = if self.f == DP_HIDDEN_BIT {
            DiyFp::new((self.f << 2).wrapping_sub(1), self.e - 2)
        } else {
            DiyFp::new((self.f << 1).wrapping_sub(1), self.e - 1)
        };
        mi.f <<= mi.e - pl.e;
        mi.e = pl.e;
        (mi, pl)
    }

    #[must_use]
    pub fn to_double(&self) -> f64 {
        debug_assert!(self.f <= DP_HIDDEN_BIT + DP_SIGNIFICAND_MASK);
        if self.e < DP_DENORMAL_EXPONENT {
            // Underflow.
            return 0.0;
        }
        if self.e >= DP_MAX_EXPONENT {
            // Overflow.
            return f64::INFINITY;
        }
        let be: u64 = if self.e == DP_DENORMAL_EXPONENT && (self.f & DP_HIDDEN_BIT) == 0 {
            0
        } else {
            (self.e + DP_EXPONENT_BIAS) as u64
        };
        f64::from_bits((self.f & DP_SIGNIFICAND_MASK) | (be << DP_SIGNIFICAND_SIZE))
    }
}

// 10^-348, 10^-340, ..., 10^340
const CACHED_POWERS_F: [u64; 87] = [
    0xfa8f_d5a0_081c_0288,
    0xbaae_e17f_a23e_bf76,
    0x8b16_fb20_3055_ac76,
    0xcf42_894a_5dce_35ea,
    0x9a6b_b0aa_5565_3b2d,
    0xe61a_cf03_3d1a_45df,
    0xab70_fe17_c79a_c6ca,
    0xff77_b1fc_bebc_dc4f,
    0xbe56_91ef_416b_d60c,
    0x8dd0_1fad_907f_fc3c,
    0xd351_5c28_3155_9a83,
    0x9d71_ac8f_ada6_c9b5,
    0xea9c_2277_23ee_8bcb,
    0xaecc_4991_4078_536d,
    0x823c_1279_5db6_ce57,
    0xc210_9436_4dfb_5637,
    0x9096_ea6f_3848_984f,
    0xd774_85cb_2582_3ac7,
    0xa086_cfcd_97bf_97f4,
    0xef34_0a98_172a_ace5,
    0xb238_67fb_2a35_b28e,
    0x84c8_d4df_d2c6_3f3b,
    0xc5dd_4427_1ad3_cdba,
    0x936b_9fce_bb25_c996,
    0xdbac_6c24_7d62_a584,
    0xa3ab_6658_0d5f_daf6,
    0xf3e2_f893_dec3_f126,
    0xb5b5_ada8_aaff_80b8,
    0x8762_5f05_6c7c_4a8b,
    0xc9bc_ff60_34c1_3053,
    0x964e_858c_91ba_2655,
    0xdff9_7724_7029_7ebd,
    0xa6df_bd9f_b8e5_b88f,
    0xf8a9_5fcf_8874_7d94,
    0xb944_7093_8fa8_9bcf,
    0x8a08_f0f8_bf0f_156b,
    0xcdb0_2555_6531_31b6,
    0x993f_e2c6_d07b_7fac,
    0xe45c_10c4_2a2b_3b06,
    0xaa24_2499_6973_92d3,
    0xfd87_b5f2_8300_ca0e,
    0xbce5_0864_9211_1aeb,
    0x8cbc_cc09_6f50_88cc,
    0xd1b7_1758_e219_652c,
    0x9c40_0000_0000_0000,
    0xe8d4_a510_0000_0000,
    0xad78_ebc5_ac62_0000,
    0x813f_3978_f894_0984,
    0xc097_ce7b_c907_15b3,
    0x8f7e_32ce_7bea_5c70,
    0xd5d2_38a4_abe9_8068,
    0x9f4f_2726_179a_2245,
    0xed63_a231_d4c4_fb27,
    0xb0de_6538_8cc8_ada8,
    0x83c7_088e_1aab_65db,
    0xc45d_1df9_4271_1d9a,
    0x924d_692c_a61b_e758,
    0xda01_ee64_1a70_8dea,
    0xa26d_a399_9aef_774a,
    0xf209_787b_b47d_6b85,
    0xb454_e4a1_79dd_1877,
    0x865b_8692_5b9b_c5c2,
    0xc835_53c5_c896_5d3d,
    0x952a_b45c_fa97_a0b3,
    0xde46_9fbd_99a0_5fe3,
    0xa59b_c234_db39_8c25,
    0xf6c6_9a72_a398_9f5c,
    0xb7dc_bf53_54e9_bece,
    0x88fc_f317_f222_41e2,
    0xcc20_ce9b_d35c_78a5,
    0x9816_5af3_7b21_53df,
    0xe2a0_b5dc_971f_303a,
    0xa8d9_d153_5ce3_b396,
    0xfb9b_7cd9_a4a7_443c,
    0xbb76_4c4c_a7a4_4410,
    0x8bab_8eef_b640_9c1a,
    0xd01f_ef10_a657_842c,
    0x9b10_a4e5_e991_3129,
    0xe710_9bfb_a19c_0c9d,
    0xac28_20d9_623b_f429,
    0x8044_4b5e_7aa7_cf85,
    0xbf21_e440_03ac_dd2d,
    0x8e67_9c2f_5e44_ff8f,
    0xd433_179d_9c8c_b841,
    0x9e19_db92_b4e3_1ba9,
    0xeb96_bf6e_badf_77d9,
    0xaf87_023b_9bf0_ee6b,
];

const CACHED_POWERS_E: [i16; 87] = [
    -1220, -1193, -1166, -1140, -1113, -1087, -1060, -1034, -1007, -980, -954, -927, -901, -874, -847, -821, -794,
    -768, -741, -715, -688, -661, -635, -608, -582, -555, -529, -502, -475, -449, -422, -396, -369, -343, -316, -289,
    -263, -236, -210, -183, -157, -130, -103, -77, -50, -24, 3, 30, 56, 83, 109, 136, 162, 189, 216, 242, 269, 295,
    322, 348, 375, 402, 428, 455, 481, 508, 534, 561, 588, 614, 641, 667, 694, 720, 747, 774, 800, 827, 853, 880, 907,
    933, 960, 986, 1013, 1039, 1066,
];

#[must_use]
pub fn get_cached_power_by_index(index: usize) -> DiyFp {
    debug_assert!(index < 87);
    DiyFp::new(CACHED_POWERS_F[index], i32::from(CACHED_POWERS_E[index]))
}

/// Returns `(power, K)`.
#[must_use]
pub fn get_cached_power(e: i32) -> (DiyFp, i32) {
    //int k = static_cast<int>(ceil((-61 - e) * 0.30102999566398114)) + 374;
    let dk = f64::from(-61 - e) * 0.301_029_995_663_981_14 + 347.0; // dk must be positive, so can do ceiling in positive
    let mut k = dk as i32;
    if dk - f64::from(k) > 0.0 {
        k += 1;
    }

    let index = ((k >> 3) + 1) as u32;
    let big_k = -(-348 + (index << 3) as i32); // decimal exponent no need lookup table

    (get_cached_power_by_index(index as usize), big_k)
}

/// Returns `(power, outExp)`.
#[must_use]
pub fn get_cached_power10(exp: i32) -> (DiyFp, i32) {
    debug_assert!(exp >= -348);
    let index = (exp + 348) as u32 / 8;
    let out_exp = -348 + index as i32 * 8;
    (get_cached_power_by_index(index as usize), out_exp)
}
