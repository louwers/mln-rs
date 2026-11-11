// Ports vendor/wagyu/include/mapbox/geometry/wagyu/almost_equal.hpp
// (Google Test's FloatingPoint<double>::AlmostEquals, kMaxUlps = 4), which
// keeps Google Test's license instead of wagyu's:
//
// Copyright 2005, Google Inc.
// All rights reserved.
//
// Redistribution and use in source and binary forms, with or without
// modification, are permitted provided that the following conditions are
// met:
//
//     * Redistributions of source code must retain the above copyright
// notice, this list of conditions and the following disclaimer.
//     * Redistributions in binary form must reproduce the above
// copyright notice, this list of conditions and the following disclaimer
// in the documentation and/or other materials provided with the
// distribution.
//     * Neither the name of Google Inc. nor the names of its
// contributors may be used to endorse or promote products derived from
// this software without specific prior written permission.
//
// THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
// "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
// LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
// A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
// OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
// SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
// LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
// DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
// THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
// (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
// OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.

const K_MAX_ULPS: u64 = 4;
const K_SIGN_BIT_MASK: u64 = 1 << 63;

/// `SignAndMagnitudeToBiased`.
fn sign_and_magnitude_to_biased(sam: u64) -> u64 {
    if K_SIGN_BIT_MASK & sam != 0 {
        // sam represents a negative number.
        (!sam).wrapping_add(1)
    } else {
        // sam represents a positive number.
        K_SIGN_BIT_MASK | sam
    }
}

/// `DistanceBetweenSignAndMagnitudeNumbers`.
fn distance_between_sign_and_magnitude_numbers(sam1: u64, sam2: u64) -> u64 {
    let biased1 = sign_and_magnitude_to_biased(sam1);
    let biased2 = sign_and_magnitude_to_biased(sam2);
    biased1.abs_diff(biased2)
}

/// `FloatingPoint<double>(x).AlmostEquals(FloatingPoint<double>(y))`: at
/// most 4 ULPs apart; false for NaNs; +0.0 and -0.0 are equal.
pub(crate) fn almost_equals(x: f64, y: f64) -> bool {
    // The IEEE standard says that any comparison operation involving
    // a NAN must return false.
    if x.is_nan() || y.is_nan() {
        return false;
    }
    distance_between_sign_and_magnitude_numbers(x.to_bits(), y.to_bits()) <= K_MAX_ULPS
}
