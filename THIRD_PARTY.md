# Third-party code and data

mln-rs is under the BSD 2-Clause License in [LICENSE.md](LICENSE.md), as
MapLibre Native is. The code it ports from other projects keeps their
licenses. Each package's `license` field gives the SPDX expression for what
it holds, and the package ships the license texts.

## Ported code

| Code | Ports | License | Text |
| --- | --- | --- | --- |
| `vendor/cheap-ruler` | [cheap-ruler-cpp](https://github.com/mapbox/cheap-ruler-cpp) | ISC | [LICENSE](vendor/cheap-ruler/LICENSE) |
| `vendor/csscolorparser` | [css-color-parser-cpp](https://github.com/mapbox/css-color-parser-cpp) | MIT | [LICENSE](vendor/csscolorparser/LICENSE) |
| `vendor/earcut` | [earcut.hpp](https://github.com/mapbox/earcut.hpp) | ISC | [LICENSE](vendor/earcut/LICENSE) |
| `vendor/feature`, `vendor/geometry` | [geometry.hpp](https://github.com/mapbox/geometry.hpp) | ISC | [LICENSE](vendor/geometry/LICENSE) |
| `vendor/geojson` | [geojson.hpp](https://github.com/mapbox/geojson-cpp) | ISC | [LICENSE](vendor/geojson/LICENSE) |
| `vendor/geojson-vt` | [geojson-vt-cpp](https://github.com/mapbox/geojson-vt-cpp) | ISC | [LICENSE](vendor/geojson-vt/LICENSE) |
| `vendor/kdbush` | [kdbush.hpp](https://github.com/mourner/kdbush.hpp) | ISC | [LICENSE](vendor/kdbush/LICENSE) |
| `vendor/nunicode` | nunicode 1.8, and its tables of Unicode data | MIT, Unicode-3.0 | [LICENSE](vendor/nunicode/LICENSE), [LICENSE-Unicode](vendor/nunicode/LICENSE-Unicode) |
| `vendor/pixelmatch` | [pixelmatch-cpp](https://github.com/mapbox/pixelmatch-cpp) | ISC | [LICENSE.txt](vendor/pixelmatch/LICENSE.txt) |
| `vendor/pmtiles` | [PMTiles](https://github.com/protomaps/PMTiles) (`pmtiles.hpp`) | BSD-3-Clause | [LICENSE](vendor/pmtiles/LICENSE) |
| `vendor/polylabel` | [polylabel](https://github.com/mapbox/polylabel) | ISC | [LICENSE](vendor/polylabel/LICENSE) |
| `vendor/protozero` | [protozero](https://github.com/mapbox/protozero), with its varint decoder from [folly](https://github.com/facebook/folly) | BSD-2-Clause, Apache-2.0 | [LICENSE.md](vendor/protozero/LICENSE.md), [LICENSE.from_folly](vendor/protozero/LICENSE.from_folly) |
| `vendor/rapidjson` | [RapidJSON](https://github.com/Tencent/rapidjson) | MIT | [LICENSE.txt](vendor/rapidjson/LICENSE.txt) |
| `vendor/shelf-pack` | [shelf-pack-cpp](https://github.com/mapbox/shelf-pack-cpp) | ISC | [LICENSE.md](vendor/shelf-pack/LICENSE.md) |
| `vendor/supercluster` | [supercluster.hpp](https://github.com/mapbox/supercluster.hpp) | ISC | [LICENSE](vendor/supercluster/LICENSE) |
| `vendor/vector-tile` | [vector-tile](https://github.com/maplibre/mvt-cpp) | ISC | [LICENSE](vendor/vector-tile/LICENSE) |
| `vendor/wagyu` | [wagyu](https://github.com/mapbox/wagyu), from the Clipper library, and Google Test's `AlmostEquals` | BSL-1.0, BSD-3-Clause | [LICENSE](vendor/wagyu/LICENSE), the header of `almost_equal.rs` |
| `crates/mln_util`: `mat2.rs`, `mat3.rs`, `mat4.rs` | [gl-matrix](https://glmatrix.net/) | Zlib | their headers |
| `crates/mln_util`: `unitbezier.rs` | WebKit's `UnitBezier` | BSD-2-Clause (Apple) | its header |
| `crates/mln_util`: `parsedate.rs` | [curl](https://curl.se/)'s date parser | curl | [LICENSE-curl](crates/mln_util/LICENSE-curl) |
| `crates/mln_text`: `ft`, `woff.rs` | [FreeType](https://freetype.org/) 2.13.3 | FTL | [LICENSE-FreeType](crates/mln_text/LICENSE-FreeType) |
| `crates/mln_text`: `hb_ft.rs`, `hb_ucd.rs`, `hb_ucd_table.rs` | [HarfBuzz](https://harfbuzz.github.io/) 8.2.0, and its tables of Unicode data | MIT-Modern-Variant, Unicode-3.0 | [LICENSE-HarfBuzz](crates/mln_text/LICENSE-HarfBuzz), [LICENSE-ICU](crates/mln_text/LICENSE-ICU) |
| `crates/mln_text`: `icu` | [ICU](https://icu.unicode.org/) 78.3 | Unicode-3.0, ICU | [LICENSE-ICU](crates/mln_text/LICENSE-ICU) |

The FreeType port's files say in their headers which FreeType functions
they port and how the port differs.

## Shipping mln-rs

An app that ships mln-rs reproduces the notices of the packages in its
binary: the licenses above and those of the crates.io dependencies. A tool
such as [cargo-about](https://github.com/EmbarkStudios/cargo-about) collects
them from the packages. The FreeType License also asks for a credit in the
app's documentation, such as:

> Portions of this software are copyright © 2024 The FreeType Project
> (www.freetype.org). All rights reserved.

## Test data

The packages' `testdata` directories hold copies of files from a MapLibre
Native checkout (`just update-testdata`): its test fixtures and those of the
libraries it vendors. They keep the terms of the project they come from and
of the map data, fonts and images in them, and are not part of the published
packages. `crates/mln_text/tests/fixtures` holds a subset of Noto Sans Khmer,
under the SIL Open Font License 1.1 (in the font's name table).
