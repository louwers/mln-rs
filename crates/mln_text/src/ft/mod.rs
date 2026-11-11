//! The subset of FreeType 2.13 (../maplibre-native/vendor/freetype) that the
//! C++ core's text code relies on, ported for exact FreeType semantics on
//! top of the font data (see each file's header).
//!
//! Portions of this software are copyright © The FreeType Project
//! (<https://freetype.org>). All rights reserved. They are under the FreeType
//! License (LICENSE-FreeType), one of the two licenses FreeType offers.

pub mod calc;
pub mod cmap;
pub mod face;
pub mod grays;
