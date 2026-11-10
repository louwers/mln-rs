# OpenFreeMap C++-vs-Rust comparison

Renders OpenFreeMap styles (Liberty, Bright, Positron, Dark, Fiord) with the
C++ `mbgl-render` and with the Rust render-test runner at the same cameras,
and compares the two with the render-test runner's pixelmatch metric and
default allowed difference (0.00015). It needs no new code: the C++ tool
fills a C++ resource cache (`cache.db`), and the Rust runner renders the
same style offline from that database through a private manifest
(`manifest.template.json`), with the C++ image as `expected.png`.

Files:

- `views.txt`: the views (style, name, lon, lat, zoom, bearing, pitch, pixel
  ratio, width, height).
- `just liberty-references [regex]`: C++ references for the views matching
  `style/name`.
- `just liberty-compare [regex]`: the Rust renders and the comparison.
- `manifest.template.json`: the render-test manifest (`@WORK_DIR@` is
  replaced by the work directory).

## Work directory (not in git)

Everything generated goes to `$LIBERTY_COMPARE_DIR` (default
`~/.cache/mln-liberty-compare`), outside the repository:

- `cache.db`: the C++ resource cache (style, TileJSON, vector and raster
  tiles, glyph PBFs, sprites), downloaded by `just liberty-references`;
- `styles/<style>.json`: the OpenFreeMap style documents;
- `tests/render-tests/<style>/<view>/{style.json,expected.png,cpp.log}`;
- `bin/render-test`: the copy of the Rust binary `just liberty-compare` runs;
- `manifest.json`, `results/manifest.html`, `results/render-tests/**/{actual,diff}.png`.

## Steps

1. A C++ `mbgl-render` built from `../maplibre-native` with the Vulkan
   backend (the C++ Bazel `//bin:render_tool` is macOS-only), inside the C++
   repo's `#linux` flake shell:

   ```
   cd ../maplibre-native
   nix develop path:$PWD#linux --command bash -c \
     'cmake --preset linux-vulkan -DCMAKE_BUILD_TYPE=Release && cmake --build build-linux-vulkan --target mbgl-render'
   ```

   (Not re-run for this README: the 2026-11-03 results below used an
   existing Release build with `MLN_WITH_VULKAN=ON` from 2026-10-27. The
   preset sets ccache as compiler launcher; add
   `-DCMAKE_CXX_COMPILER_LAUNCHER= -DCMAKE_C_COMPILER_LAUNCHER=` if the
   shell has none.) The binary runs inside this repo's `.nix-shell`
   (created by the justfile on NixOS), with lavapipe.
2. The Rust runner: `just liberty-compare` builds it
   (`cargo build -p mln-rs-render-test --bin render-test`).
3. References (network: tiles.openfreemap.org):

   ```
   MBGL_RENDER=<path>/mbgl-render just liberty-references             # all views
   MBGL_RENDER=<path>/mbgl-render just liberty-references '^liberty/'
   ```

   Each view takes 1-2 s once its resources are cached.
   `LIBERTY_COMPARE_VIEWS=<file>` uses another view list (same format).
4. Comparison (offline):

   ```
   just liberty-compare             # all prepared views
   just liberty-compare 'liberty/'  # one style
   ```

   The summary ("N passed / M failed") is printed at the end; per-view
   diffs, actual and diff images are in `results/manifest.html`.

## Results (2026-11-03, tree ~17:50)

- Liberty: **18 of 22 views pass** at 0.00015 (world z1.5, Amsterdam
  z13/15.5/17.2, NYC z14, Tokyo z13/16, Cairo, Tel Aviv (Hebrew), Bangkok,
  Delhi, Seoul, Athens, Moscow, Berlin @2x, Paris @2x, London bearing 45,
  SF pitch 50). Labels, road shields, line labels, RTL and CJK text match
  C++. Added later: rome18, istanbul12 (bearing 170), nyc15p45r2 (pitch 45
  @2x) pass; paris14p60 (pitch 60, 3D buildings) misses at 0.00026, again
  only fill-outline edge pixels (21 of 26 Liberty views). The `sw-*`
  sweep (20 more places and zooms z0-z19: Rio, Sydney, Cape Town, Mumbai,
  Beijing, Reykjavik, Mexico City, Dubai, Jerusalem, the antimeridian,
  ...) passes 15 of 20; sw-ams8 (0.00047), sw-rio7 (0.00046), sw-syd11
  (0.00018), sw-syd6 (0.0013) and sw-cairo6p (0.0031, pitch 55) are again
  fill-outline pixels (forest and coastline outlines). In total 36 of 46
  Liberty views pass, no render errors. Check of the outline explanation:
  with `fill-antialias: false` on every fill layer (style passed to
  mbgl-render as a file, `-s style.json`), sw-syd6 and sw-cairo6p match
  C++ with diff 0.
- The 4 misses are all the same known fill-outline difference, not a port
  bug: europe5 (0.0016), ams10 (0.0005) and alps9 (0.0003) differ in
  single antialiasing pixels along fill outlines (forest/park polygons),
  and nyc12pitch (0.0015, pitch 60) lacks the `park` layer's 1px
  `fill-outline-color` around a park far from the camera. The reference
  `mbgl-render` is C++ **Vulkan**, which draws fill outlines as 1px line
  primitives; the port follows the C++ **WebGPU** backend, which (like
  Metal) triangulates fill outlines, and those thin out under pitch
  (see the platform-macos expectations in the render-test manifest).
- Bright, Positron, Dark, Fiord (Amsterdam z13, NYC z14, Europe z4.5,
  Tokyo z15 pitch 45, Paris @2x): **20 of 20 pass**.
