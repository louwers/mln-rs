// A minimal unsigned big integer for exact decimal/hex -> binary floating
// point conversion in cstdlib.rs (glibc does the same with its mpn
// routines). Little-endian u32 limbs, no leading zero limbs.

use std::cmp::Ordering;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Big {
    limbs: Vec<u32>,
}

impl Big {
    pub fn from_u64(v: u64) -> Self {
        let mut b = Big { limbs: vec![v as u32, (v >> 32) as u32] };
        b.trim();
        b
    }

    fn trim(&mut self) {
        while self.limbs.last() == Some(&0) {
            self.limbs.pop();
        }
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    pub fn bit_len(&self) -> u64 {
        match self.limbs.last() {
            None => 0,
            Some(&top) => (self.limbs.len() as u64 - 1) * 32 + (32 - u64::from(top.leading_zeros())),
        }
    }

    /// self = self * m + a
    pub fn mul_add_small(&mut self, m: u32, a: u32) {
        let mut carry = u64::from(a);
        for limb in &mut self.limbs {
            let v = u64::from(*limb) * u64::from(m) + carry;
            *limb = v as u32;
            carry = v >> 32;
        }
        if carry != 0 {
            self.limbs.push(carry as u32);
        }
    }

    pub fn mul_pow10(&mut self, mut n: u64) {
        while n >= 9 {
            self.mul_add_small(1_000_000_000, 0);
            n -= 9;
        }
        if n > 0 {
            self.mul_add_small(10u32.pow(n as u32), 0);
        }
    }

    pub fn shl(&mut self, n: u64) {
        if self.is_zero() {
            return;
        }
        let words = (n / 32) as usize;
        let bits = (n % 32) as u32;
        if bits != 0 {
            let mut carry = 0u32;
            for limb in &mut self.limbs {
                let v = *limb;
                *limb = (v << bits) | carry;
                carry = v >> (32 - bits);
            }
            if carry != 0 {
                self.limbs.push(carry);
            }
        }
        if words != 0 {
            let mut v = vec![0u32; words];
            v.extend_from_slice(&self.limbs);
            self.limbs = v;
        }
    }

    pub fn cmp_big(&self, other: &Big) -> Ordering {
        if self.limbs.len() != other.limbs.len() {
            return self.limbs.len().cmp(&other.limbs.len());
        }
        for (a, b) in self.limbs.iter().rev().zip(other.limbs.iter().rev()) {
            if a != b {
                return a.cmp(b);
            }
        }
        Ordering::Equal
    }

    /// self -= other; requires self >= other.
    pub fn sub_assign(&mut self, other: &Big) {
        let mut borrow = 0i64;
        for i in 0..self.limbs.len() {
            let b = i64::from(*other.limbs.get(i).unwrap_or(&0));
            let mut v = i64::from(self.limbs[i]) - b - borrow;
            if v < 0 {
                v += 1 << 32;
                borrow = 1;
            } else {
                borrow = 0;
            }
            self.limbs[i] = v as u32;
        }
        debug_assert_eq!(borrow, 0);
        self.trim();
    }

    /// Returns `(floor(self / d), self % d != 0)` for a quotient below 2^64.
    pub fn div_small_quotient(mut self, d: &Big) -> (u64, bool) {
        debug_assert!(!d.is_zero());
        let mut q = 0u64;
        let nb = self.bit_len();
        let db = d.bit_len();
        if nb >= db {
            let top = nb - db;
            debug_assert!(top < 64);
            let mut i = top as i64;
            while i >= 0 {
                let mut ds = d.clone();
                ds.shl(i as u64);
                if self.cmp_big(&ds) != Ordering::Less {
                    self.sub_assign(&ds);
                    q |= 1u64 << i;
                }
                i -= 1;
            }
        }
        (q, !self.is_zero())
    }
}
