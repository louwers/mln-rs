# Deviations from the C++ behaviour

Each entry: C++ location, what Rust does differently, why, and how tests
account for it.

- **`fmin`/`fmax` with signed zeros** (`util::min`/`util::max`, `util::clamp`).
  C++ calls the platform libm, whose choice between `-0.0` and `+0.0` is not
  specified. The port implements glibc's choice (return the first argument
  when the two compare equal). Difftests run on Linux/glibc.
- **Render-test outputs** (`render-test/runner.cpp`, `render_test.cpp`). C++
  writes `actual.png`/`diff.png` next to each test's style.json and the HTML
  report next to the manifest; the Rust runner writes them under `--output`
  (required) as `<test id>/actual.png` etc.,
  because the C++ tree is read-only and baselines are never overwritten.
  For the same reason `--update` is rejected. `--shuffle` reproduces the
  libstdc++ (Linux) order, not libc++'s.
- **Render-test comparison with a larger expected image**
  (`TestRunner::checkRenderTestResults`). C++ calls pixelmatch with the
  expected image's width and height over the actual image's buffer; if the
  expected image has more pixels, that reads and writes out of bounds. The
  Rust runner counts every pixel as different (difference 1) and tries the
  next `expected*.png`. Smaller or equal pixel counts behave as in C++.
- **Render-test runner robustness**. C++ terminates on exceptions it does not
  catch (`Map::setSize`, decoding a corrupt `expected.png`, an invalid
  `setCenter` LatLng, an unknown `setTileLodMode`); the Rust runner errors
  that test with the message and continues. A panic while creating or using
  the map errors the test and drops that map. `getValidPath` resolves
  against each manifest's directory (C++ caches the first one in a
  `static`; the runner parses one manifest per process). JSON parse errors
  carry serde_json's message instead of rapidjson's "... at offset N".
- **PNG codec** (`platform/default/src/mln/util/png_{reader,writer}.cpp`,
  `compression.cpp`). Decoding uses the `png` crate with libpng's transform
  chain (expand, strip 16, gray to RGB, alpha filler), so the pixels match;
  libpng's error texts after "failed to read invalid png: '" differ.
  Encoding writes the same chunks, but flate2 (miniz_oxide) compresses
  differently from zlib: same pixels, different bytes. That changes
  compressed `probeFileSize` metrics (they measure `util::compress`); no
  pixel comparison is affected. JPEG decoding is TODO(port) (WebP: see [dem-heatmap] below).
- **[expression] `fmin`/`fmax` signed zeros, verified** (entry above):
  `//difftest/util:math_difftest` (`observed_signed_zero_behaviour` and the
  full ±0/NaN/inf cross product through `util::min`/`max`/`clamp`) shows the
  oracle returning the first argument on equal operands, at `-c fastbuild`
  and `--config=oracle-opt` (clang 20, glibc). `mln_math::{fmin, fmax,
  fminf, fmaxf}` implement exactly that.
- **[expression] x86-64 results for C++ undefined behaviour in
  CSSColorParser** (`vendor/csscolorparser/csscolorparser.cpp`,
  `clamp_css_byte`). `int64_t i = ::round(i)` overflows for values within 512
  of `INT64_MAX` (e.g. `rgb(9223372036854775807,0,0)` after `strtoll`
  saturates), and `uint8_t(NaN)` for `rgb(nan%,…)`. Both are UB; the port
  reproduces what clang emits on x86-64 (`cvttsd2si` gives `INT64_MIN`, so
  the channel clamps to 0, not 255; NaN gives 0). On arm64 the C++ saturates
  (255). Covered by `//difftest/util:csscolorparser_difftest`
  (`observed_quirks`, edge cases) on Linux x86-64.
- **[expression] C library number parsing is glibc's**
  (`csscolorparser::cstdlib::{strtof, strtod, strtoll}`, used by
  CSSColorParser and `mln_util::string::stof`). Correct rounding, glibc's
  ERANGE rules (overflow; underflow = tiny after rounding and inexact) and
  end pointers, "C" locale. `strtoll` is glibc ≥ 2.38's C23
  `__isoc23_strtoll` (what C++ gets with `_GNU_SOURCE`): base 0 and 2 accept
  `0b`. Other C libraries were not checked. Covered by
  `//difftest/util:cstdlib_difftest`.
- **[expression] `Padding::to_array`** returns `[f64; 4]` (seed API), C++
  `toArray()` returns `std::array<float, 4>`; same values. `to_float_array`
  has the C++ type.
- **[expression] JSON number parsing and object members**
  (`ConversionTraits<const JSValue*>`, src/mln/style/rapidjson_conversion.hpp).
  `Convertible` reads serde_json (with `float_roundtrip`, so numbers are
  correctly rounded like rapidjson's `kParseFullPrecisionFlag`, which the C++
  expression-test runner uses). Style JSON in C++ is parsed with
  `Parse<0>` (normal precision), which can be one ULP off for numbers with
  more than ~15 significant digits or large exponents; the integer literal
  `-0` is the unsigned 0 in rapidjson and `-0.0` in serde_json; with
  duplicate object keys rapidjson's `objectMember` finds the first, serde_json
  keeps the last. Not covered by tests yet.
- **[expression] Object iteration order** (`expression::Value`'s and
  `mapbox::base::Value`'s `std::unordered_map`, `Match` branches). The port
  uses `BTreeMap` (key order); C++ iterates in libstdc++ hash order. Affects
  the key order of `stringify`/`to-string`/`serialize` output for objects and
  the `each_child` order of `match` branches (C++ `Match::serialize` sorts,
  so serialization is unaffected there). The corpus runner compares objects
  key-wise (`deepEqual`), and `//difftest/expression` sorts keys in its
  canonical form.
- **[expression] Collator and `upcase`/`downcase`** (platform/default
  collator.cpp and string_stdlib.cpp, nunicode). Exact through the
  `//vendor/nunicode` port (see the nunicode entry for the invalid UTF-8 that
  nunicode writes for some 4-byte case mappings: the Rust strings hold U+FFFD
  there). `//difftest/expression` compares the expressions against the C++
  platform; the 7 `collator/*` corpus tests pass.
- **[expression] `number-format`** (platform/default `number_format.cpp`,
  ICU `NumberFormatter`). The port reimplements what ICU does for the root
  and English locales: double-conversion's shortest digits (ties to even),
  half-even rounding to max-fraction-digits, padding to min-fraction-digits,
  `,` grouping, the sign of negative numbers that round to zero, and for a
  currency ICU's currency precision with the en-US symbol of USD, EUR, JPY,
  GBP, CNY, INR, KRW, CAD, AUD (other codes: `CODE` + U+00A0). Other locales
  and currencies differ. `//difftest/expression` compares it against ICU for
  the root/en/en-US locales (generated numbers and digit counts, four
  currencies) and the corpus.
- **[expression] x86-64 results for C++ undefined behaviour in expressions**:
  `static_cast<int>`/`<int64_t>` of out-of-range doubles (value.cpp
  `writeJSON`, `slice`, `number-format` digit counts, `match` input
  rounding) reproduce `cvttsd2si` (INT_MIN / INT64_MIN). `Value::isSafeInteger
  (double)` of |x| >= 2^64 saturates (C++ UB, compiler dependent).
  `slice` with a negative normalized start index reads out of bounds in C++;
  the port starts at 0. `slice` with an erroring `fromIndex` dereferences the
  error in C++ (it checks the expression, not the result); the port returns
  the error. TODO(port) comments mark each.
- **[expression] Exceptions out of `evaluate`**: the
  `Interpolator<VariableAnchorOffsetCollection>` throws `std::runtime_error`
  for mismatched collections and `toVariableAnchorOffset` throws
  `bad_variant_access` for non-numeric offsets; both propagate out of
  `Expression::evaluate` in C++. The port returns an `EvaluationError` with
  the message for the former and panics for the latter.
- **[expression] Expression tests runner** (`expression-test/`). `UPDATE`
  mode and the HTML report are not ported; `-f` filters by substring, not
  regex; a panic in one test errors that test instead of aborting the run;
  fixture JSON errors carry serde_json's message. Legacy functions go
  through `mln_style`'s `convert_function_to_expression_for_type`.
- **[style] Layer/light observers** (style/layer.cpp, light.cpp,
  style_impl.cpp). C++ layers and the light hold a `LayerObserver*` /
  `LightObserver*` (the owning `Style::Impl`) and every setter calls
  `onLayerChanged` / `onLightChanged`. A Rust layer cannot point back into
  the collection that owns it, so `Style::Impl::get_layer_mut` /
  `get_light_mut` return guards that run `onLayerChanged` /
  `onLightChanged` on drop if the layer's or light's impl `Arc` was
  replaced (every notifying C++ setter replaces it). Several setter calls
  through one guard notify once; the final state is the same.
  `style::Observer` methods take `&self` (`Rc<dyn Observer>`), so an
  observer cannot read the style back from inside `on_update` (the C++
  TransitionOptionsPublishUpdates test does; the port reads it afterwards).
- **[style] Projection observer** (style/projection.cpp, style_impl.cpp). As for layers and the light (above), the C++ `ProjectionObserver*` that `Projection::setType` notifies is replaced by the guard `Style::Impl::get_projection_mut` returns, which runs `onProjectionChanged` on drop if the projection's impl `Arc` was replaced. Map.StyleExpiredWithProjection sets the projection both through the guard and with `set_projection`, and checks that the expired style's reload keeps it.
- **[style] Sky observer** (style/sky.cpp, style_impl.cpp). As for layers, the light and the projection (above), the C++ `SkyObserver*` is replaced by the guard `Style::Impl::get_sky_mut` returns, which runs `onSkyChanged` on drop if the sky's impl `Arc` was replaced, and `setSky(std::unique_ptr<Sky>)` takes an `Option<Sky>`, whose `None` removes the sky as C++'s `nullptr` does. Style.SkyPresenceAndReplacement sets, replaces and removes the sky.
- **[globe] The globe is always built** (util/constants.hpp `GLOBE_ENABLED`, `MLN_WITH_GLOBE` in BUILD.bazel and CMakeLists.txt). C++ compiles the globe out without `MLN_WITH_GLOBE`, a flag that exists to keep iOS binaries small, and then draws a style's globe as Mercator and says so in a warning. PLAN.md keeps build configurations out of the port, so the port always has the globe, as C++ does with the flag on (its default), and has no warning. The port runs every globe test, as C++ does with the flag on; C++'s skips for a build without it (the gtest filter in test/src/mln/test/test.cpp, the projection check in render-test/render_test.cpp) have no counterpart.
- **[style] JSON syntax errors** (style/parser.cpp, util/rapidjson.cpp
  `formatJSONParseError`). The style is parsed with serde_json, so the
  message after "Failed to parse style: " is serde_json's ("expected value
  at line 1 column 2"), not rapidjson's `GetParseError_En(code) + " at
  offset N"`. Valid styles parse the same. Duplicate object keys: rapidjson
  `operator[]`/`HasMember` find the first, serde_json keeps the last value
  (at the first key's position).
- **[style] Sprites and sources without storage** (style_impl.cpp
  `Style::Impl::parse`). Until FileSource/SpriteLoader are ported, the style
  acts as if it had a file source whose sprite requests fail: no "sprite"
  -> the C++ "non-existent sprite" path (loaded); with sprites ->
  `onSpriteRequested` + `onSpriteError` per sprite (C++ marks failed named
  sprites loaded), synchronously inside `load_json`. Source conversion
  returns "source type '<t>' is not ported yet" for every type (the parser
  logs it and skips the source, as for an invalid one), and the raster
  layer type is not registered (logged "Unsupported layer type! ...").
  render-tests: only background-opacity/overlay (M4) depends on this.
- **[style] Layer manager split** (layermanager/). `mln_style` holds the
  style half (`LayerFactory::createLayer`, `LayerManager::createLayer`); the
  C++ factories also create render layers, buckets and layouts, which the
  renderer does by `LayerTypeInfo` (mln_renderer is above mln_style).
- **[gfx] Crate cut around the drawable path** (renderer/layer_group.hpp,
  renderer/render_pass.hpp, renderer/paint_parameters.hpp). `LayerGroupBase`,
  `TileLayerGroup`, `LayerGroup` and the `mln::RenderPass` bitmask live in
  `mln_gfx::renderer` (the backends implement them); the layer groups keep no
  tweaker list (`LayerTweaker` needs style types, so `addLayerTweaker` /
  `runTweakers` live in mln_renderer); gfx/webgpu see `mln::PaintParameters`
  through the `mln_gfx::PaintParameters` trait; `render(RenderOrchestrator&,
  ...)` drops the unused orchestrator; `renderTileClippingMasks` takes tile
  ids instead of `RenderTiles`. `visitDrawables` has no try/catch (C++
  removes drawables whose callback throws and reports `onRenderError`).
  Behaviour on the render path is unchanged.
- **[gfx] Hash values** (util::hash users: `ProgramParameters::definesHash`,
  `ShaderGroup::propertyHash`/`getShaderName`, `ColorMode::hash`, the WebGPU
  pipeline and stencil hashes). The combination is boost's `hash_combine`,
  but the leaf hashes (std::hash of strings and ints) differ, and the defines
  map is ordered (C++: unordered_map order). These values are only cache
  keys and shader-group names ("BackgroundShader#<hex>"), never rendered.
- **[gfx] Shader groups store programs** (gfx/shader_group.hpp). C++ stores
  `shared_ptr<gfx::Shader>` and callers `static_pointer_cast` to
  `ShaderProgramBase`; the port stores `ShaderProgramBasePtr` (every
  registered shader is a program). Ported shader_registry gtests use stub
  programs that implement the trait.
- **[webgpu] wgpu instead of Dawn/wgpu-native** (src/mln/webgpu,
  headless_backend.cpp). Same calls through wgpu 30's Rust API, with these
  differences: the headless instance uses `WGPU_BACKEND` if set, else Vulkan
  on Linux (C++ wgpu-native: all backends, HighPerformance adapter) so tests
  run on lavapipe; WGSL compile and pipeline creation errors are caught with
  validation error scopes and give "no module"/"no pipeline" as the C++ null
  checks do, while any other validation error panics (wgpu's default
  handler; Dawn logs and invalidates the command buffer); the readback blocks
  with `Device::poll(Wait)` (C++: WaitAny / a 1000 x 1 ms poll loop); uniform
  buffers get their initial data by `queue.write_buffer` instead of
  `mappedAtCreation` (same contents at the next submit); a render pass
  without a stencil clear passes `stencil_ops: None` (C++: Undefined
  load/store ops); render passes are recorded with `forget_lifetime`, so
  wgpu rejects finishing an encoder with an open pass (C++ code never does).
- **[webgpu] Registered shader types** (renderer_backend.cpp `initShaders`).
  C++ registers all 31 WebGPU shader groups; the port registers those whose
  attribute/texture tables (src/mln/shaders/webgpu/*.cpp) are ported:
  BackgroundShader, BackgroundPatternShader, ClippingMaskProgram, and (M3,
  layers) CircleShader, FillShader, FillOutlineShader, FillPatternShader,
  FillOutlinePatternShader, FillOutlineTriangulatedShader, LineShader,
  LineGradientShader, LinePatternShader, LineSDFShader,
  LineGradientSDFShader. The WGSL of all of them is extracted
  (tools/extract_wgsl.rs). Others arrive with their layers.
- **[expression] nunicode `b4_utf8` writes invalid UTF-8**
  (vendor/nunicode/include/libnu/utf8_internal.h). For 4-byte codepoints with
  bit 12 set, the C puts that bit into the third byte (mask `0x001F00`
  instead of `0x000F00`), so `platform::uppercase`/`lowercase` and the
  collator's `unaccent` emit invalid UTF-8 for mappings to such codepoints
  (e.g. Warang Citi U+118A0 <-> U+118C0). `//vendor/nunicode` reproduces the
  bytes (`uppercase_bytes`, `lowercase_bytes`, `unaccent_bytes`,
  `collator_compare` use them); the `String` functions (`uppercase`,
  `lowercase`, `unaccent`) decode them lossily (U+FFFD), because a Rust
  `String` cannot hold them. `//difftest/nunicode` checks the bytes exactly
  and the `String` against the lossy decoding of the C++ bytes.
- **[render] Layers without a source and the annotation source**
  (render_orchestrator.cpp `createRenderTree`). C++ handles background
  layers (and the background-as-clear-colour shortcut) while iterating over
  the style's *first source*; every C++ style has at least one, the
  annotation source that `AnnotationManager::onStyleLoaded` adds (plus its
  `org.maplibre.annotations.points` symbol layer). Annotations are not
  ported, so a Rust style may have no sources; the Rust loop then runs once
  without a source, handling only the source-less layers, exactly as the C++
  iteration over the annotation source does for them. When annotations are
  ported this becomes the C++ loop again. The output differs from C++ only
  in the absent annotation layer/source (empty without annotations).
- **[render] Style observer calls during `&mut Style`** (map_impl.cpp). C++
  `Map::Impl` handles style::Observer calls synchronously inside
  `Style::loadJSON`, layer mutations etc. In Rust the map hands out the
  style through a `StyleMut` guard; observer calls made while it is held are
  queued and replayed, in order, when it drops (before any other map call).
  Calls made at other times (sprite/source callbacks) are handled at once.
- **[render] LayerTweaker lists of layer groups** (layer_group.cpp
  `addLayerTweaker`/`runTweakers`). LayerTweaker is a renderer type and the
  layer groups live in mln_gfx, so the orchestrator keeps each group's weak
  tweaker list, filled through a change request that `processChanges`
  executes before the tweakers first run in the same frame (same order of
  effects as C++).
- **[storage] Actors stay on the thread that established them**
  (actor/mailbox.cpp, actor_ref.hpp, established_actor.hpp). A C++ message
  holds a raw reference to its target, which another thread may destroy
  (after `Mailbox::close` waits for a running `receive`). Safe Rust cannot
  send that reference, so the target lives in a registry of its thread
  (`mln_actor::LocalObject`) and messages are closures over `&O`; `close()`
  and `receive()` run on that thread, and the single mailbox mutex gives the
  C++ guarantees (no message after `close`, pushes to a closed mailbox are
  dropped). Actors scheduled on a thread pool (`Actor<O>` with
  `Scheduler::GetBackground()`) are a TODO(port); file sources, their
  requests and `util::Thread` actors behave as in C++. `schedule_and_reply_value`
  keeps the reply closure on the calling thread for the same reason.
- **[storage] `FileSourceManager::getFileSource` runs the factory without
  its lock** (C++: a recursive mutex held across the factory, which calls
  back into the manager for MainResourceLoader). On one thread the result
  is the same; if two threads create the same (type, options) file source at
  once, the first one registered is returned to both.
- **[storage] DatabaseFileSource's tile server options**
  (database_file_source.cpp `DatabaseFileSourceThread`). C++ reads them from
  the online file source unconditionally; the port does too, and falls back
  to the ResourceOptions it was created with only when no Network factory is
  registered (C++ would dereference null). (Updated by gtests-storage.)
- **[storage] OfflineDatabase** (offline_database.cpp; write path, regions,
  merge, eviction ported by gtests-storage). `util::compress` is flate2's
  miniz_oxide, not zlib: compressed bytes and therefore stored sizes differ
  (1024 zero bytes: 23 bytes, zlib 17), which shifts `put`'s returned size,
  region completed sizes and the ambient-cache accounting by those bytes
  (OfflineDatabase.PutReturnsSize is ignored for it; a zlib-backed flate2
  would remove the difference). Logic errors C++ catches with `catch (...)`
  (`std::invalid_argument` / `std::domain_error` from a stored region
  definition) log "Can't <action>" without the debug `assert(false)`.
  `Query::get<std::string>` of a REAL column formats with Rust's `{}`
  instead of SQLite's `%!.15g` (no such columns in the schema's text or blob
  fields). `util::decompress` error texts are flate2's. The read path is
  covered by `//difftest/storage` on every row of both cache databases.
- **[storage] SQLite wrapper** (sqlite3.cpp). rusqlite underneath. A
  `Database` shares its connection with its `Transaction`s (C++: both point
  at the heap `DatabaseImpl`), as an `Arc<Mutex<Connection>>`, so
  `Database::conn()` returns a guard. `Query` binds before its first `run()`
  only (TODO(port): `reset`/`clearBindings` and re-binding). `exec` runs
  every statement to completion like `sqlite3_exec` (rusqlite's
  `execute_batch` steps each statement once). No `SQLITE_CONFIG_LOG` hook,
  no `setTempPath`. test::SQLite3TestFS (the test VFS) is not ported: it
  needs unsafe FFI, so the 4 tests using it are ignored.
- **[storage] Asset, Local, MBTiles and PMTiles file sources**
  (asset_file_source.cpp, local_file_source.cpp, local_file_request.cpp,
  mbtiles_file_source.cpp, pmtiles_file_source.cpp). The resource and client
  options live next to the file source's thread behind a mutex (C++: in the
  thread object, read back with a blocking `ask`); same values. A
  `std::filesystem::status` failure other than "not found" (e.g. a name too
  long) reads the file and answers "Cannot read file", as C++ does. MBTiles:
  where C++ lets an exception escape its thread (`Database::open`, a query,
  `std::stoi` of a non-number, `util::decompress`), the port answers the
  request with an `Other` error carrying the message, and `std::stoi`/
  `std::strtod` of text without a number read 0. MBTiles/PMTiles TileJSON
  documents are built member by member and written with //vendor/rapidjson's
  Writer (duplicate member names kept, as rapidjson's `AddMember` does).
  pmtiles.hpp's reading half is ported as //vendor/pmtiles.
- **[storage] OfflineDownload** (offline_download.cpp). In
  //crates/mln_storage_offline (it parses styles and covers regions with
  tiles, which mln_storage can't depend on), with the canonicalize* half of
  util/mapbox.cpp (Rust `regex` for std::regex; the patterns are built the
  same way). The download shares the database (`Rc<RefCell<OfflineDatabase>>`)
  and the online file source (`Arc<dyn FileSource>`) instead of holding
  references; its callbacks hold it weakly. `OfflineRegionObserver` methods
  take `&self`: the download calls them re-entrantly. `invokeCancellable` is
  a task on the current RunLoop that a dropped request cancels.
  TODO(port): DatabaseFileSource's offline-region API (list/get/create/
  merge/update/delete/invalidate regions, region status, observers and
  download state, which create OfflineDownloads), reset/pack/invalidate/
  clear ambient cache, setMaximumAmbientCacheSize; no test in test/storage
  goes through them.
- **[storage] The HTTP test server** (test/src/mln/test/http_server.cpp,
  cpp-httplib on 127.0.0.1:3000). `mln_test_server` (tools/test-server) has
  the same routes on an httpmock server, one per test on a free port, so
  route state (`/cache`'s counter, the fail-once routes) starts fresh for
  each test. The `/online/` fixtures name `127.0.0.1:3000`; they are served
  (and `online_fixture` reads them) with the test's server address instead.
  Query parameters split as httplib's do (`a=b=c` is `a` = `c`).
- **[map] MapSnapshotter renders on the caller's thread**
  (platform/default map_snapshotter.cpp). C++ renders on a renderer thread
  and forwards the renderer's observer calls through an actor; the port's
  renderer is single-threaded, so it renders on the snapshotter's thread.
  The snapshot callback still runs from the run loop, as C++'s actor
  delivers it, and the image, attributions and transforms are gathered
  there rather than in the still image callback (nothing renders in
  between). The callback is `FnMut`: like C++, a snapshot started while one
  is rendering calls back with an error and then again with the result.
- **[sprite] Sprite JSON parsing** (sprite_parser.cpp `parseSprite`, rapidjson
  `Parse<0>`). serde_json instead of rapidjson: the message after "Failed to
  parse JSON: " is serde_json's (C++: "Invalid value. at offset 0" etc.); with
  duplicate keys rapidjson visits every member and serde_json only the last
  value; invalid UTF-8 is a parse error (rapidjson does not validate); the
  integer literal `-0` is a double in serde_json (so `"x": -0` warns, C++ reads
  0). Everything else, incl. warnings, errors and pixels, is covered by
  `//difftest/storage` (fixtures, cache-database sprites, generated JSON).
- **[style] Sprites through the SpriteLoader** (supersedes the style entry
  "Sprites and sources without storage" for sprites). `Style::new(px)` is the
  C++ style with a null file source: a parsed style reports the sprite error
  "Unable to find resource provider for sprite url." and is not loaded, as in
  C++. `Style::with_file_source(..)` loads sprites through the SpriteLoader.
  `loadURL` with a file source is a TODO(port) (it reports a
  StyleLoadException "TODO(port): ..."); C++ requests and parses the URL.
- **[map] Map thread pool** (map_impl.cpp). C++ passes
  `frontend.getThreadPool()` (the renderer backend's tagged background pool)
  to the style; `RendererFrontend` has no thread pool yet, so Map::Impl uses
  the background pool with a fresh tag. Only the grouping for
  `waitForEmpty` differs.
- **[storage] StubFileSource timer** (test/src/mln/test/stub_file_source.cpp,
  tests only). The asynchronous stub polls from an AsyncTask re-armed every
  millisecond while requests are pending, not a 1 ms `util::Timer`
  (RunLoop timers are a TODO(port)).
- **[codegen] Style code generator** (scripts/generate-style-code.mjs,
  style-code.mjs, style-spec.mjs, the .ejs templates; ported as
  //tools/style-codegen). Same decisions, with these implementation
  differences: (1) properties are sorted by bytes instead of
  `Intl.Collator("en-US")`; for names of lowercase ASCII letters, digits and
  '-' ICU's root order is the byte order, and the generator refuses any other
  name; (2) default colors are parsed with the vendored csscolorparser port
  instead of the npm one, which agree for alpha 0 and 1, the only alphas of
  the spec defaults; the generator refuses fractional alphas. Checked against
  the C++ generated headers: every tag's base class (including the
  attribute/uniform names), default-value initializer, `name()`, the property
  sets' order, kPaintPropertyCount, the Property enum, the property tables,
  the setPropertyInternal conversion groups and the LayerTypeInfo values
  agree for all 11 layer types; //difftest/style_codegen checks the values.
- **[codegen] Generated property tags** (src/mln/style/layers/*_layer_properties.hpp).
  The data-driven tags don't carry the `A`/`U` template arguments
  (`attributes::radius, uniforms::radius`); the C++ base class with them is
  each tag's doc comment, for the renderer's drawables to look up.
  `Unevaluated::stringify` / `Layer::Impl::stringifyLayout` are not ported
  (TODO(port); nothing in the C++ core calls stringifyLayout).
  `PossiblyEvaluated::evaluate<P>(z, feature, ...)` is `evaluate`,
  `evaluate_canonical`, `evaluate_state`, `evaluate_images`,
  `evaluate_images_canonical`; the argument combinations C++ has no overload
  for (feature state or images with a non-data-driven property, any
  cross-faded property) are `unreachable!` instead of compile errors.
- **[codegen] FormatSectionOverrides** (symbol_layer_impl.hpp). The style
  half (`hasOverrides`, `hasOverride`, `hasPaintPropertyDifference`) is in
  `layers::symbol_layer_impl::SymbolLayerPaintPropertyOverrides`, written out
  for the one overridable property (text-color) instead of folded over a
  type list; `setOverride(s)` / `updateOverride(s)` (render side,
  `FormatSectionOverride`) are TODO(port) with RenderSymbolLayer (M5), so the
  ported SymbolLayerOverrides gtest checks the style half only.
- **[codegen] //difftest/style_codegen inputs**. For `resolvedImage`
  properties the sample set has no constant expression such as
  `["case", true, "a", "b"]`: the `image` coercion is not constant-folded,
  and an expression that is feature-, zoom- and runtime-constant but not a
  literal hits `assert(false)` in conversion/property_value.cpp (a C++
  precondition).
- **[tile] Strings from tiles are UTF-8** (vendor/vector-tile, protozero
  `get_string`, `VectorMVTTileLayer::getName`). C++ keeps layer names, keys
  and string values as `std::string` bytes; `mln::Value::String` and the
  layer name are Rust `String`s, so invalid UTF-8 becomes U+FFFD
  (`from_utf8_lossy`). Layer lookup, layer order and the key comparison of
  `getValue` work on the raw bytes as in C++, so only these differ: string
  values and property keys with invalid UTF-8 (lossy), two keys that are
  equal only after the lossy conversion (C++ keeps both, the port the first),
  and a layer whose name is not UTF-8 cannot be requested through `&str`
  (style `source-layer`s are JSON strings, so never in practice).
  `//difftest/tile` normalizes the C++ strings UTF-8-lossy before comparing;
  test/fixtures/map/invalid_utf8 matches after normalization.
- **[tile] protozero wire-type assertions** (`protozero_assert(has_wire_type(..))`
  in pbf_reader.hpp `get_*()`). `vector_tile.hpp` never checks wire types,
  so a known field with the wrong wire type aborts a debug (non-NDEBUG) C++
  build; release C++ decodes it as if the type matched. The port always
  does what release C++ does. The data-independent preconditions ("call
  next() before accessing field value") are `debug_assert!`s, and debug
  builds reset `tag()` to 0 after a get that skips bytes, as C++ does.
  `//difftest/tile` compares inputs that reach such an assertion (11 fixture
  files: glyph PBFs and non-MVT blobs; and all byte-mutated tiles) only with
  `--config=oracle-opt`.
- **[tile] Exceptions out of the GeometryTile\* trait methods**
  (vector_mvt_tile_data.cpp). C++ `getLayer`, `getFeature`, `getValue`,
  `getProperties` and `getGeometries` throw on malformed tiles (protozero
  exceptions, `std::out_of_range`, `std::runtime_error`); the
  `mln_tile_id::geometry_tile_data` traits have no error channel. The
  `VectorMVTTile*` types have `try_*` methods returning
  `Result<_, vector_tile::Error>` (same exception type and `what()`), and
  the trait methods panic with the `what()` text where the C++ exception
  would escape. The C++ GeometryTileWorker catches everything around
  parsing (`catch (...)` -> `onError`); its port must use `catch_unwind` or
  the `try_*` methods there.
- **[tile] x86-64 results of undefined casts and overflows on the tile
  path**: `getGeometries` with extent 0 (scale = inf) converts NaN with
  `static_cast<int16_t>`, which clang compiles to cvttss2si (0x80000000)
  keeping the low 16 bits, i.e. 0, which Rust's `as` also gives;
  `convertGeometry` (lng/lat to tile coordinates) casts out-of-range and
  NaN doubles to int16 before its clamp, reproduced as cvttsd2si + low 16
  bits; `signedArea` and wagyu's int32 sums/products wrap. Covered by
  `//difftest/tile` (extent 0, NaN/inf/huge lng/lat, ±32768 coordinates).
- **[tile] libstdc++ algorithm order** (tile/geometry_tile_data.cpp
  `limitHoles`, vendor/wagyu). The order `std::nth_element` leaves the
  kept holes in, the result of wagyu's `std::stable_sort` with
  `intersect_list_sorter` (a ULP-tolerant comparator, not a strict weak
  ordering) and the order of equal keys in wagyu's `std::unordered_multimap`
  are libstdc++'s; the port reproduces those implementations
  (`mln_tile::stl_algo::nth_element`, `//vendor/libstdcxx`'s
  `stable_sort_by`, wagyu's `ConnectionMap`). C++ built against libc++ or
  MSVC's library can give a
  different hole order or clipper output. Checked against the Linux oracle.
- **[text] HBShaper on HarfRust instead of HarfBuzz 8.2.0 (HB_TINY) + hb-ft**
  (src/mln/text/harfbuzz_impl.cpp). The C++ core shapes with its vendored
  HarfBuzz 8.2.0 built with `HB_TINY`, on an hb-ft font (FreeType glyph
  mapping and advances, scale 24 px in 26.6). The port shapes with HarfRust
  0.15 (HarfBuzz 15.0.0 behaviour) and reproduces what it can of that setup:
  hb-ft's nominal/variation glyph and advance callbacks on a port of
  FreeType 2.13's charmaps, sizing and `FT_Get_Advance`
  (`crates/mln_text/src/{ft,hb_ft.rs}`; `//difftest/text:ft_difftest`:
  19.1M charmap lookups and 36k advances identical on 145 fonts), hb-ft's
  scale, HarfBuzz 8.2.0's own Unicode 15.0 tables for the script
  segmentation (`hb_ucd_table.rs`, generated; every codepoint identical,
  `:ucd_difftest`), and HB_TINY's compiled-out features by hiding the
  tables HarfBuzz never reads (morx, mort, kerx, kern, ankr, trak, feat;
  HB_NO_AAT, HB_NO_OT_KERN), dropping GSUB/GPOS FeatureVariations
  (HB_NO_VAR) and passing `frac=0 numr=0 dnom=0` (HB_NO_OT_SHAPE_FRACTIONS).
  `//difftest/text:shaping_difftest` shapes 407 strings (107 hand-written,
  300 generated) with 145 fonts (HarfBuzz's perf/API test fonts and the
  render tests' font-faces fonts): 55446 of 59015 cases are bit-identical.
  Every one of the 3569 others is minimized to a reproducer and has one of
  causes 1-6 (counts at fastbuild and `--config=oracle-opt` alike):
  1. *Unicode data* (1691 cases, 22 minimal texts): the text has a
     codepoint unassigned in Unicode 15.0 (HB_NO_UCD_UNASSIGNED gives those
     its neighbours' properties) or whose script or combining class differs
     between HarfBuzz 8.2.0 (Unicode 15.0) and HarfRust (Unicode 18.0):
     29210 codepoints differ in script, 80 in combining class, nearly all
     unassigned in 15.0. Reproducer: U+05CF alone (any font without it):
     C++ advance 0 (treated as a mark), Rust the .notdef advance.
  2. *Fallback mark positioning* (1649 cases, 166 minimal texts):
     HB_NO_OT_SHAPE_FALLBACK turns it off in C++; HarfRust always runs it
     for fonts without (usable) GPOS — Hebrew needs a `hebr` GPOS script —
     and, with no glyph extents (hb-ft's extents are unused by HB_TINY, so
     the port returns none), it zeroes the advance of every non-spacing
     mark after a base, also marks GDEF does not class as marks (typically
     .notdef). Reproducer: Noto Sans devanagari.ttf, "ו\u{5b9}": C++
     advances 14.40625 14.40625, Rust 0 14.40625 (visual order).
  3. *Vowel constraints* (79 cases, 1 text): HB_NO_OT_SHAPER_VOWEL_CONSTRAINTS
     in C++; HarfRust inserts U+25CC. Reproducer: "अॅ" (U+0905 U+0945),
     every font: 2 glyphs in C++, 3 in Rust.
  4. *HarfRust bug, rounding* (27 cases, 12 texts): HarfBuzz (8.2.0 and
     15.0.0) rounds scaled anchor positions with `roundf` = `floor(x + .5)`;
     HarfRust's `Scale::scale_x_f` uses `f32::round`, so -n.5 ends one unit
     lower. Reproducer: perf/fonts/Roboto-Regular.ttf "Ҙ̥" (U+0498 U+0325):
     the mark's anchor delta is -10 units * 0.75 = -7.5: HarfBuzz -7,
     HarfRust -8 (HBShapeAdjust y_offset 0.109375 vs 0.125).
  5. *HarfRust bug, fallback spaces* (114 cases, 5 texts): HarfBuzz sizes
     U+2000..U+200A etc. from its scale, `(x_scale + n/2) / n`; HarfRust
     scales `(upem + n/2) / n`. Reproducer: Noto Sans devanagari.ttf (upem
     1000) U+2004: C++ 512/64 = 8.0, Rust 511/64.
  6. *FreeType's synthesized charmap* (9 cases, 1 font): RanaKufi-Regular.subset.otf
     has no Unicode cmap; FreeType's CFF driver synthesizes one from the
     glyph names (psnames + the Adobe Glyph List), which is not ported
     (TODO(port) in ft/face.rs), so its characters map to .notdef.
  4 and 5 are HarfRust bugs, to be fixed upstream. A local patch fixed both
  and changed nothing else (`crates/mln_text/patches/harfrust_hb_rounding.patch`,
  removed in favour of upstream HarfRust; see the git history). The difftest has no cause for them, so
  `shaping_matches_harfbuzz` is ignored until HarfRust fixes them. 1-3 need
  HarfRust features (HB_TINY-like switches or Unicode tables) that do not
  exist.
- **[text] Glyph rasterization** (FreeTypeFace::rasterizeGlyph,
  freetype.cpp). The port loads hinted outlines with skrifa
  (`HintOptions::auto_fallback`, grayscale: FreeType's FT_LOAD_DEFAULT engine
  choice) and renders them with a port of FreeType's smooth rasterizer
  (`ft/grays.rs`: ftgrays.c's FT_INT64 paths and ftsmooth.c's overlap
  oversampling) into `ft_glyphslot_preset_bitmap`'s box. Differences come
  from skrifa's hinting engines vs FreeType 2.13's. Measured by
  `//difftest/text:rasterize_difftest` on 8814 glyphs (up to ~400 per
  font, 145 fonts): metrics identical for 8189 (92.9%), bitmaps
  identical for 5991 (68.0%), bitmap size differs for 414 (4.7%); in
  same-size bitmaps 41237 of 3757221 pixels (1.1%) differ, by 18.35 on
  average, at most 255. Per engine, fonts with no differing pixel: TrueType
  bytecode 37 of 58, PostScript 49 of 51, autohinter 20 of 29. The SDF step
  (`util::transformRasterToSDF`, ported as `mln_util::tiny_sdf`) is
  bit-identical on all 8814 C++ bitmaps. Embedded-bitmap glyphs (CBDT/
  sbix/EBDT without outlines) are TODO(port) (empty glyph).
- **[geometry] x86-64 results for out-of-range `double` -> integer casts**
  (geojson-vt `InternalTile::transform` `static_cast<int16_t>(round(..))`,
  supercluster `getTile` (`point<int16_t>` from doubles, `uint32_t z2 =
  pow(2, z)`), geojson-vt `splitTile` / kdbush `select` (`uint32_t`/`size_t`
  from double), earcut `zOrder`). Out-of-range values are UB in C++; the
  ports reproduce clang's x86-64 code (`cvttsd2si` into a 32-bit register
  for `int16_t`/`int32_t`, keeping the low bits; 64-bit for `uint32_t`;
  `//vendor/libstdcxx` `cvt`), not Rust's saturating `as`. Reachable with
  unclipped `geoJSONToTile` (custom geometry sources with `clip: false`) at
  high zooms; `//difftest/geometry:geojson_vt_difftest` covers it on Linux
  x86-64. arm64 C++ saturates instead.
- **[geometry] mapbox::geojson on serde_json** (geojson.hpp
  `geojson_impl.hpp`, `//vendor/geojson`). `convert` takes a
  `serde_json::Value` (C++: a rapidjson value), so the JSON-level
  differences of the expression entry above apply: the last of duplicate
  keys wins (C++ `FindMember`/`emplace`: the first), the literal `-0` is the
  double -0.0 (C++: unsigned 0), `parse` rounds every number correctly (C++
  `parse` uses rapidjson's default precision, which can be one ULP off for
  long decimals) and its syntax errors carry serde_json's text (C++:
  "<offset> - <rapidjson message>"). Where the C++ calls a rapidjson accessor
  on the wrong type (a non-number coordinate, a non-array line string in a
  MultiLineString, a non-string "type" in an error message), rapidjson
  asserts in debug builds and reads garbage in release builds; the port
  returns an error ("coordinates must be numbers", the container error,
  "" as the type name). `to_json` (C++ `convert(geojson, allocator)`) writes
  NaN/infinite doubles as `null` (a rapidjson value can hold them). Property
  maps are key-ordered (`BTreeMap`), so `stringify`/`to_json` write members
  in key order where C++ writes `std::unordered_map` order; values and every
  other byte are the same (`//difftest/geometry:geojson_difftest` compares
  byte-exact when no property object has more than one key, structurally
  otherwise).
- **[geometry] Handles instead of pointers** (shelf-pack `Bin*`, earcut
  `Node*`). `ShelfPack` returns `BinRef`s (shelf and bin index; equal iff
  the C++ pointers are equal) read through `ShelfPack::bin`; `stats()` and
  `shelves()` expose two private members for tests. Same packing, reuse and
  refcounts (`//difftest/geometry:shelf_pack_difftest`).
- **[geometry] std::stable_sort without a temporary buffer**
  (`//vendor/libstdcxx`). libstdc++ falls back to `__inplace_stable_sort` /
  `__stable_sort_adaptive_resize` when it cannot allocate the `(n + 1) / 2`
  buffer; the port only has the full-buffer path (an allocation failure
  aborts). Results are identical whenever the C++ allocation succeeds.
- **[shaping] BiDi on unicode-bidi instead of ICU's ubidi**
  (platform/default/src/mln/text/bidi.cpp; policy change of 2026-11-03).
  Levels, paragraphs and visual runs come from `unicode-bidi` 0.3.18
  (UAX #9 at Unicode 16.0); the C++ uses ICU 78.3's ubidi (Unicode 17.0).
  The port writes runs as `ubidi_writeReordered`/`ubidi_writeReverse` with
  UBIDI_DO_MIRRORING | UBIDI_REMOVE_BIDI_CONTROLS do (RTL runs reversed by
  code point and mirrored with ICU 78.3's u_charMirror data; ZWNJ, ZWJ, LRM,
  RLM, LRE..RLO, LRI..PDI dropped). Known differences, not yet measured
  against ICU: ICU treats CR LF as one paragraph separator, unicode-bidi
  splits after each (an extra line break inside "\r\n"); ICU's line-level
  L1 handling (setTrailingWSStart, the trailing-WS run's odd bit) and
  bracket-pair quirks may differ from unicode-bidi's; Bidi_Class of code
  points new in Unicode 17. The bidi.test.cpp gtests pass unchanged.
  Arabic letter shaping (`applyArabicShaping`) is a port of ICU 78.3's
  u_shapeArabic for the options bidi.cpp passes (no crate implements it).
  `getShaping`'s `static_cast<uint32_t>` of a negative image display size
  and `static_cast<int32_t>` of an infinite line count are UB in C++; the
  port's `as` casts saturate.
- **[mlt] MLT tiles are decoded by mlt-core 0.22.1, not the C++ decoder**
  (src/mln/tile/vector_mlt_tile_data.cpp on vendor/maplibre-tile-spec/cpp).
  `crates/mln_tile/src/mlt.rs` fills the C++ decoder's output model from
  mlt-core; `vector_mlt_tile_data.rs` ports the glue. On every fixture tile
  both decoders read, the MLT difftest found no difference (layers, types,
  ids, property variants and bits, scaled geometry, triangles, log).
  `//difftest/mlt` was removed 2026-11-04 (user decision: mlt-core is assumed
  correct; the measurements here are from its last run, and mbgl-core's
  maplibre-tile-spec module is now fetched from GitHub by `MODULE.bazel`
  instead of the vendor submodule). Known differences:
  1. *DOUBLE columns stored as 4-byte floats* (fixtures `14-4357-6245`,
     `counties-7-37-48`, `mapbox.mapbox-streets-v7/4-4-7`, `4-5-7` and their
     copies in `metrics/cache-style.db`): each has one optional DOUBLE
     column (`len`, `pop_density`, `area`) whose data stream holds
     `num_values` binary32 values (`byte_length = 4 * num_values`). The spec
     defines DOUBLE as IEEE754 binary64; the C++ decoder has a compatibility
     branch ("tilesets encoded before double support was added",
     decode/property.hpp) that widens the floats; mlt-core rejects the tile
     ("decodable stream size expected 2n, got n"), so the Rust tile has no
     layers and logs "MLT parse failed: ...". Not an RLE check, as an earlier
     note said. Minimal reproducer (31 bytes, one point, DOUBLE "v" = 1.5f):
     `1e 01 01 61 80 20 02 04 1a 01 76 02 10 02 01 01 00 13 42 02 02 02 04
     10 00 01 04 00 00 c0 3f` (`mlt_difftest.rs` `DOUBLE_AS_FLOAT_TILE`).
     No render or query test uses these tiles (the five `source-replace` MLT
     render tests load others), so it is a recorded tolerance.
  2. *Malformed tiles* (measured by `mlt_difftest` `mutations`, not
     asserted): mlt-core rejects invalid UTF-8, extent 0, empty layer names
     and inconsistent offsets that the C++ accepts or reads out of bounds;
     the C++ rejects integer streams with physical technique NONE, varints
     over 32 bits and trailing layer bytes that mlt-core accepts; the C++
     always applies componentwise delta to vertex streams, mlt-core follows
     the stream's logical encoding. mlt-core also caps decoded memory at
     64 MiB per tile (C++: no cap). Error texts after "MLT parse failed: "
     are mlt-core's.
  3. Ported C++ quirks: MULTIPOLYGON geometries start with one empty ring per
     ring (`lines.emplace(n)` + `back_inserter`); multi-points report
     `FeatureType::LineString`; `getProperties` after a throwing column keeps
     the partial map in wire order (C++: hash order). `getFeature` past the
     end panics (C++: unchecked index).
- **[layers] Buckets hold plain vectors; drawables get copies**
  (renderer/buckets/*.hpp, render_{circle,fill,line}_layer.cpp). C++ buckets
  own `std::shared_ptr<gfx::VertexVector>`/`IndexVector`s that the drawables
  share (and that the backend attaches GPU buffers to). A Rust bucket is
  built on the worker thread and shared as `Arc<dyn Bucket>` (Send + Sync),
  so it holds `Vec`s, and the render layer copies them into the
  render-thread `Rc` gfx vectors when it builds a tile's drawables (once per
  tile and bucket, as C++ builds the drawables). Same vertex/index data on
  the GPU. Feature-state updates of binders (`Bucket::update`) are a
  TODO(port) (no M3 render test uses feature state outside patterns).
- **[layers] Layouts keep feature indices** (layout/circle_layout.hpp,
  pattern_layout.hpp). C++ layouts keep the `unique_ptr<GeometryTileFeature>`s
  their filter accepted; a Rust feature borrows its tile layer, so the
  layouts keep the indices (and sort keys) and get the features again in
  `createBucket`. Same features, same order.
- **[layers] gfx/fill_generator lives in mln_renderer**
  (src/mln/gfx/fill_generator.cpp): it includes the renderer's bucket
  headers (vertex types, `FillBucket::layoutVertex`,
  `LineBucket::layoutVertex`), so it is `mln_renderer::gfx::fill_generator`;
  the generic `gfx/polyline_generator.cpp` is `mln_gfx::polyline_generator`.
- **[layers] LineAtlas keyed by value** (geometry/line_atlas.cpp). C++ keys
  the dash textures by a hash of the dash arrays and cap and ignores hash
  collisions; the port keys by the values (float bits). Same textures
  without the collision case.
- **[layers] Fill outlines with data-driven color/opacity**
  (render_fill_layer.cpp, `MLN_TRIANGULATE_FILL_OUTLINES`). As in the C++
  WebGPU backend, data-driven outlines are drawn as `gfx::Lines(2)`, which
  WebGPU rasterizes 1px wide, so `fill-outline-color/{property-function,
  zoom-and-property-function}` differ from the OpenGL baselines; C++ WebGPU
  ignores both (metrics/ignores/platform-macos-webgpu.json), the port lists
  them in tools/render-test/ignores/rust-port.json with that reason.
- **[layers] `within`: degenerate input** (style/expression/within.cpp,
  util/geometry_util.cpp, `GeoJSONFeature(feature, canonical)` in
  expression.cpp). Rings and lines with fewer than two points walk no
  edges (C++ computes `size() - 1` on a `size_t` and reads out of bounds),
  int64 tile-coordinate arithmetic wraps (overflow is UB in C++), and a
  wagyu error in `fixupPolygons` while evaluating an expression on a
  GeoJSON feature keeps the unfixed rings instead of throwing out of
  `evaluate`. Valid input gives the same results (expression corpus
  within/* and the within render tests).
- **[mlt] Spec compliance over C++ compatibility (user decision, 2026-11-03).**
  Four fixture tiles (metrics/integration/tiles/14-4357-6245.mlt,
  counties-7-37-48.mlt, mapbox.mapbox-streets-v7/4-4-7.mlt and 4-5-7.mlt, and
  the same tiles in metrics/cache-style.db) store a DOUBLE column as 4-byte
  floats. The MLT spec defines DOUBLE as an 8-byte IEEE754 value; only the C++
  decoder's legacy path accepts them, and mlt-core rejects them. The port
  stays spec compliant: the MLT difftest (removed 2026-11-04, see the [mlt]
  entry above) ignored these tiles by name (`NON_SPEC_TILES`) and failed on
  any other tile with the same pattern. To do (upstream, maplibre-native):
  re-encode these fixtures with real doubles. No render or query test uses
  them.
- **[sources] Sources and tiles are handles to `Rc`'d state** (style/source.cpp,
  sources/*, tile/tile.cpp, geometry_tile.cpp, renderer/render_source.cpp).
  C++ sources, tiles and render sources are reached from asynchronous
  callbacks through raw pointers / WeakPtrs (`observer->onSourceLoaded(*this)`,
  the worker's `ActorRef<GeometryTile>`, TileLoader request callbacks).
  The port keeps the mutable state in cells behind an `Rc` that the callbacks
  reach through a `Weak`; every `Source`/`Tile` method takes `&self`.
  `Style::Impl`'s `sources.update(source)` (onSourceLoaded/Changed) happens
  when the impls are read (`get_source_impls` picks up every changed source
  impl). Observers get the tile's `TileBase` (C++ `Tile&`) and the render
  source's id (C++ `RenderSource&`). A `RenderTile` copies the tile id and
  hold-for-fade state when prepared (C++ keeps a `Tile&`).
- **[sources] Tile worker threads** (tile/geometry_tile_worker.cpp,
  actor/actor.hpp). GeometryTileWorker is a pool actor
  (`mln_actor::actor::Actor`: object behind a mutex, one message at a time
  on the thread pool); its replies reach the tile through the tile's mailbox
  on the render thread. With `synchronousUpdate` the worker runs on the
  render thread (C++ OptionalActor); a call into the synchronous worker
  while it is running (C++ re-enters it) is logged and dropped (TODO(port)).
  C++ `catch (...)` in the worker's handlers catches panics too
  (`catch_unwind`), and `Err`s from `Layout::create_bucket`. GeoJSON tiles
  use a sequenced scheduler as C++ (`get_sequenced`, ported next to the
  GeoJSON source), geojson-vt behind a mutex. TileCache drops released
  tiles on the render thread (C++: on the thread pool).
- **[sources] Unported tile-pipeline parts**: the ImageManager (a tile's
  image request, e.g. for *-pattern dependencies, is answered at once
  without images, so the tile completes without its pattern), glyph
  requests and symbol layouts, FeatureIndex's grid and queries, feature
  state, debug buckets/drawables, raster/raster-dem/image/custom sources
  (raster sources convert but have no render source: the orchestrator skips
  their layers), `canonicalizeSourceURL`/`canonicalizeTileset` (mapbox://
  URLs). C++ throws out of the style parse for a clustered GeoJSON source
  with non-point features; the port reports it as the source conversion
  error. JSON syntax errors in TileJSON/GeoJSON responses carry serde_json's
  message. groupLayers keeps groups in first-appearance order (C++:
  unordered_map order; only the feature index sort order, used by queries,
  depends on it). Out-of-range numbers in GeoJSON options (`static_cast` UB
  in C++) saturate. Covered by `//difftest/sources` (source conversion,
  TilePyramid tile sets, GeoJSON tile features) and the render tests.
- **[glfw] GLFW platform: Wayland first** (glfw_view.cpp `glfwInitHint(GLFW_PLATFORM,
  GLFW_PLATFORM_X11)`). C++ forces X11 because Dawn lacks Wayland; wgpu
  presents to Wayland, so `mln-glfw` tries Wayland, then X11
  (`MLN_GLFW_PLATFORM=x11|wayland` forces one). GLFW events are polled and
  dispatched from the window's queue right after `glfwPollEvents` (C++ runs
  callbacks inside it); `swap()` does not call `glfwPollEvents`.
- **[glfw] Window surface backend** (glfw_webgpu_backend_impl.ipp →
  `mln_webgpu::SurfaceBackend`): the window reports its framebuffer size
  instead of the backend calling `glfwGetFramebufferSize`; a pending resize
  is applied before acquiring the next swapchain texture (C++ renders one
  frame into the old-size texture with the new-size viewport, which Dawn
  reports as a validation error); a zero-size (minimized) window keeps the
  old configuration; wgpu validation errors are logged (the Dawn branch's
  `logUncapturedError`) instead of panicking; the surface gets COPY_SRC when
  supported (port-only `--readback`). The view skips a frame whose
  swapchain texture can't be acquired (timeout/occluded) and retries on the
  next tick; C++ renders it without a color attachment.
- **[glfw] Style::loadURL response** (style_impl.cpp `loadURL` lambda capturing
  `this`). The FileSource callback cannot reach the `&mut Style::Impl` that
  the Map owns, so it queues the response and calls the port-only
  `Observer::on_style_response`; the Map then runs
  `Impl::process_style_responses` (the lambda body) and replays the observer
  calls. Same order of effects, one RunLoop task later.
- **[glfw] HTTP file source on ureq** (platform/default http_file_source.cpp,
  curl). Transfers run on 20 worker threads (blocking ureq) instead of a curl
  multi handle on the RunLoop; results come back to the requesting thread
  through a mailbox, so callbacks run on the same thread as in C++. ureq
  sends `Accept-Encoding: gzip` and decodes gzip (curl: "gzip, deflate");
  transport errors carry ureq's message (C++: `curl_easy_strerror: buffer`),
  mapped to `Connection` for DNS/connect/timeout/IO errors, else `Other`;
  `Last-Modified`/`Expires` parse with the vendored parsedate (libcurl's
  `curl_getdate` differs only after 2037). Connections race a host's
  addresses with curl's Happy Eyeballs delay (200 ms), but in RFC 8305
  order (families alternate, one new attempt per delay) where curl runs one
  attempt per family at a time; ureq alone tries them one after the other,
  so an IPv6 address that drops packets stalled each new connection for the
  OS connect timeout. Status mapping, caching headers, conditional requests
  and the user agent match C++. Not observable in the app beyond error
  texts.
- **[glfw] OnlineFileSource**: request sets are ordered by request id, not
  pointer; ResourceTransform is a TODO(port); `normalizeSourceURL`'s
  "You must provide API key" exception becomes an error response.
- **[glfw] util::Timer on the port's RunLoop** (`mln_actor::Timer`): timers
  run first in `run_once` and `run()` sleeps until the next one, like libuv;
  millisecond truncation and the negative-timeout-never-fires quirk are kept.
- **[glfw] mln-glfw command line and features**: hand-parsed options (C++'s
  duplicate `-b`/`-F` short flags go to benchmark/showFeatures' places: `-b`
  benchmark, bearing and fov long-only). Not ported (TODO(port), logged
  when used): annotations, snapshots, queries, custom/GeoJSON demo sources,
  extrusion/route/free-camera demos, location indicator, rendering stats,
  render-test writer, settings load/save, SIGINT handling, database reset.
  Port-only: `--readback FILE.png`, `--quit-after SECONDS`,
  `--gesture-test`, and one summary log line of drawn/skipped layers.
- **[conformance] Raster drawables get their own tile's texture**
  (render_raster_layer.cpp `update`). C++ reuses one DrawableBuilder for all
  tiles that need new drawables in a frame and calls `setTextures` only while
  the builder has no texture yet; builders keep their textures across
  `flush`, so every tile built in the same frame after the first is drawn
  with the first tile's image (seen when several raster tiles finish
  loading between two frames). The port creates a builder per tile, so each
  tile shows its own image, as the baselines do. Textures live in the
  render layer per bucket id (C++: `RasterBucket::texture2d`), because
  buckets are shared across threads; two raster layers on one source
  therefore have separate textures (C++ shares one, and the last
  `setSamplerConfiguration` wins). Existing tile drawables are not handed
  the (unchanged) static vertex attributes again every frame; they are
  rebuilt when the bucket's mask geometry changed, as in C++.
- **[conformance] PatternAtlas texture after the atlas grows**
  (pattern_atlas.cpp `upload`, webgpu/texture2d.cpp `upload`). C++ uploads
  the grown atlas image into the existing smaller texture, which Dawn
  rejects with a validation error (wgpu: a fatal error). The port creates a
  new texture of the new size. Only reachable when a second pattern no
  longer fits the 64x64 start size.
- **[conformance] `fill-extrusion-rounded-corner-distance`**
  (geometry_tile_data.cpp `roundPolygonCorners`, fill_extrusion_bucket.cpp).
  TODO(port): the port extrudes the unrounded polygon and logs one
  warning. No render test sets the property.
- **[symbols] Symbol guards not ported** (`layout/symbol_instance.cpp`,
  `SymbolBucket::check`, MLN_SYMBOL_GUARDS). C++ checks sentinel values
  around SymbolInstance members to detect memory corruption and drops
  corrupted instances; safe Rust has no such corruption, so the checks
  (and `forceFail`/`checkIndex`/the symbol ErrorSink reports) are absent.
  Not observable in tests.
- **[symbols] Symbol drawables copy the bucket's vertex data**
  (`render_symbol_layer.cpp`). C++ drawables share the SymbolBucket's
  vertex/opacity/index vectors and see placement's updates through
  `updateModified`. The port gives drawables copies and, when the bucket's
  change counters move (placement opacities, dynamic positions,
  sortFeatures, feature state), replaces those attributes / indices of the
  tile's existing drawables (C++ `updateTileDrawable`). Same pixels; the
  changed vectors are re-uploaded as new GPU buffers.
- **[symbols] DynamicTexture GPU textures** (`gfx/dynamic_texture.cpp`,
  `webgpu/dynamic_texture.cpp`). The shelf pack and queued uploads are
  shared with the workers behind a mutex as in C++; the wgpu texture is a
  render-thread object (`Rc`), held through a `LocalObject` created in the
  first upload (C++ WebGPU `deferredCreation`) and freed with the
  DynamicTexture on the render thread (and by `~DynamicTextureAtlas`).
  Not observable.
- **[symbols] Font faces** (style "font-faces", `text/glyph_manager.cpp`
  HarfBuzz shaping). Ported, on mln_text's HBShaper (HarfRust; see the
  [text] entries for its measured differences from HarfBuzz).
- **[symbols] merge_lines keys** (`layout/merge_lines.cpp`). C++ keys its
  line-end maps on `util::hash(text, x, y)` (a `size_t` hash_combine); the
  port keys on the exact `(text, x, y)` tuple. Identical unless two C++
  hashes collide (where C++ would merge unrelated lines). `util::dist` on
  `int16` points is computed without the C++ `int` overflow (UB only for
  extreme coordinates).
- **[dem-heatmap] DEM border backfill runs in `RenderRasterDEMSource::update`**
  (`render_raster_dem_source.cpp` `onTileChanged`). C++ backfills a
  raster-dem tile's 1px border from its renderable neighbours (and theirs
  from it) when the tile reports a change; the port's tiles share one
  observer that can't reach the tile pyramid, so the source's `update`
  (every frame, before the layers' prepare/update) runs the same backfill
  for every renderable tile whose neighbour mask is not complete. Every
  pair of renderable neighbours ends up backfilled both ways before the
  prepare pass, as in C++. Same pixels (all hillshade-*/color-relief tests
  that load pass).
- **[dem-heatmap] Hillshade prepare render targets are per layer**
  (`render_hillshade_layer.cpp`, `HillshadeBucket::renderTarget`). C++
  keeps the prepare render target in the shared bucket, so a second
  hillshade layer on the same source reuses the first layer's prepared
  texture. Buckets are `Send + Sync` and render targets are render-thread
  `Rc`s, so each layer keeps its own per bucket id, re-prepared when the
  bucket's backfill counter (C++ `renderTargetPrepared = false`) moves.
  The prepare pass does not depend on layer paint properties, so the
  textures are identical (`combinations/hillshade-translucent--hillshade-translucent`
  passes); the second layer does one more offscreen pass per tile.
- **[dem-heatmap] Hillshade/color-relief drawables are rebuilt, not
  re-attributed, when the tile mask changes** (`render_hillshade_layer.cpp`,
  `render_color_relief_layer.cpp` `updateExisting`). C++ re-applies the
  shared vertex attributes (and, for color-relief, new DEM textures from
  the current DEM image) to existing drawables every frame. The port's
  drawables hold copies, so it re-applies (hillshade) or rebuilds
  (color-relief) only when the bucket's masked geometry changed, and keeps
  one DEM texture per bucket and backfill generation. Same pixels.
- **[dem-heatmap] WebP decoding on `image-webp`**
  (platform/default/src/mln/util/webp_reader.cpp, libwebp). `decodeWEBP`
  uses the image-webp crate (prefer-a-crate rule): lossless VP8L images,
  e.g. the render tests' `*.ocean.webp` DEM tiles, decode to the same
  pixels (both color-relief/hillshade combinations that use them pass);
  lossy VP8 images use its bilinear chroma upsampling, libwebp's default
  "fancy" upsampling, not compared pixel by pixel (no render test uses
  lossy WebP). Failures return an empty image with the C++ warnings.
- **[query] Combined query results in style order, topmost layer first**
  (render_orchestrator.cpp `queryRenderedFeatures`). C++ concatenates the
  per-layer results while iterating `filteredLayers`, an
  `std::unordered_map` (libstdc++ hash order; C++ ignores the tests where
  the order shows, e.g. query-tests/fill-translate/multiple-layers,
  mapbox-gl-native#12701). The port goes from the topmost style layer down,
  the order GL JS (which wrote the expectations) returns. Results within a
  layer keep the C++ order (tiles by z, y, wrap, x; features by descending
  sort index).
- **[query] FeatureIndex internals** (geometry/feature_index.cpp). The
  `RefIndexedSubfeature` strings are interned `Arc<str>`s instead of
  references into the index's containers (`unordered_set`/`unordered_map`
  node stability), so the index can be built on a worker and shared.
  `addFeature` fetches the source layer, feature and feature state once per
  indexed feature (C++: lazily at the first queried layer of the bucket;
  the values are the same for every layer). Malformed geometry
  (`convertFeature` throws, uncaught in C++) drops the feature.
- **[query] Query-test JSON** (render-test/parser.cpp `toJSON(features)`).
  The actual JSON is written by serde_json's pretty printer instead of
  rapidjson's PrettyWriter: same layout (two-space indent, `"key": value`),
  number spelling may differ. It is only parsed back for the `deepEqual`
  comparison (numbers compared after `stripPrecision`), written to
  actual.json and shown in the report's diff. Property maps are written in
  key order (C++: `unordered_map` order). A query test whose render or
  query already errored is reported with that error instead of being
  re-checked (C++ would overwrite the message with "Invalid size for
  actual image").
- **[query] Per-test time limit in the runner** (port-only; render-test has
  none). `--timeout=<seconds>` (default 60, 0 disables): a test still
  running after the limit is reported as errored, "Test timed out after N
  s", and the run continues with a fresh map. A watchdog thread posts a
  task to the test thread's RunLoop that unwinds out of the test
  (`resume_unwind`), caught by the runner's existing `catch_unwind`; so it
  only catches tests that keep turning the RunLoop (every wait in the
  runner and HeadlessFrontend does), not a hang inside one task.
  `--self-check` runs without a limit.
- **[query] Runner tile mode** (render-test/runner.cpp, `MapMode::Tile`).
  A tile that fails to render errors the test; C++ sets "Failed rendering
  tile: ..." and returns from `run` without checking the results, so the
  test counts as passed. The label cut-off/duplication report writes keys
  with `String::from_utf16_lossy` (C++: `codecvt_utf8<char16_t>`, UCS-2);
  the messages only differ for astral characters.
- **[symbols] GLFW surface present mode** (`glfw_webgpu_backend_impl.ipp`
  uses FIFO when vsync'ed). FIFO blocks acquire/present on Wayland while
  the window is hidden (no frame callbacks), which froze the GLFW app's
  event loop when the user switched windows; the port uses MAILBOX when the
  surface supports it (still vsync'ed, never blocks) and FIFO otherwise.
- **[text-conformance] `icon-text-fit/textFit-grid-long`** (render test,
  `PositionedIcon::applyTextFit`). The code matches C++; the cell with
  `textFitWidth: proportional` stretches the icon box to half-pixel edges,
  and each C++ backend rasterizes them differently (GL/Linux baseline 54
  rows, Metal 53, iOS and the port 52; C++ Vulkan fails the test too:
  metrics/ignores/platform-linux-vulkan.json, windows-vulkan.json). Kept
  in tools/render-test/ignores/rust-port.json with that reason.
- **[text-conformance] OpenFreeMap vs. C++ Vulkan `mbgl-render`**
  (tools/liberty-compare). Views that differ from the C++ Vulkan reference
  do so only in fill-outline pixels: the port draws fill outlines
  triangulated like the C++ WebGPU/Metal backends, C++ Vulkan/GL draw 1px
  line primitives. With `fill-antialias: false` the same views match with
  diff 0. Symbols (labels, shields, line labels, RTL, CJK) match C++.
- **[gtests-core] A pool actor dropped from inside its own message**
  (actor/mailbox.cpp `close()` with the recursive receiving mutex,
  test/actor/actor.test.cpp DestructionAllowedInReceiveOnSameThread). C++
  destroys the object during that message (the message's `this` dangles
  until it returns). The port's `Actor<O>` (crates/mln_actor/src/actor.rs)
  closes the mailbox immediately (no later message runs, as in C++) and
  drops the object right after the message returns, on the same thread:
  a running `&mut O` can't be dropped. Only the timing of the object's
  destructor differs.
- **[gtests-core] `LRU`, `StringIndexer` storage** (util/lru_cache.hpp,
  util/string_indexer.cpp, ported for their gtests): the LRU finds an item's
  list position by a linear scan instead of a map of list iterators, and the
  StringIndexer keeps owned `String`s instead of one char buffer re-indexed
  on reallocation. Same results; LRU `touch`/`remove` are O(n).
