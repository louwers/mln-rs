---
name: style-spec-sweep
description:
  Find the MapLibre style spec features mln-rs doesn't draw, or draws
  differently from the spec, against the latest spec. Use when asked for a style
  spec sweep, a parity audit, or to refresh the list of spec gaps.
---

# Style spec sweep

A gap is a spec feature that a style can use but that mln-rs doesn't draw as the
spec says. Whether a property is declared tells you little: a declared property
can still be drawn wrongly, for example in the wrong units or without one of its
enum values. Judge each feature against its spec text and its render tests.

## Start from the latest spec

`mise run update:style-spec <sha>` with maplibre-style-spec's latest `main`
commit, then `mise run generate:style`. Read the whole diff of
`tools/style-codegen/v8.json`, including changes to `doc` text: a property's
meaning can change while its name stays the same.

## Where things live

- `tools/style-codegen/src/spec.rs`: the properties that are removed or narrowed
  (`zoom_only`) because mln-rs doesn't draw them. Each entry is a known gap.
- `crates/mln_style/src/layers/*_layer_properties.rs`, `light_properties.rs`,
  `sky_properties.rs`: the generated properties. `crates/mln_style/src/sources`
  holds the sources, and `style_json.rs` the root properties.
- `crates/mln_style/src/expression`: the expressions. `parsing_context.rs` lists
  the ones it parses.
- `crates/mln_engine/src/layers/render_*_layer.rs`, `buckets/`, `layout/`,
  `text/`, `placement/`: where the properties are drawn.
- `tools/render-test/testdata/metrics/integration/`: the render and query tests,
  from MapLibre Native plus the GL JS tests ported since.
  `tools/render-test/ignores/*.json` and
  `tools/render-test/testdata/metrics/ignores/*.json` list the tests that fail,
  with the reasons. `tools/expression-test` holds the expression tests.

## What to look for

- **Spec entries mln-rs drops or narrows** (`spec.rs`), and any expressions,
  sources or root properties it doesn't parse.
- **Declared but never read**: a property whose field the renderer reads nowhere
  outside its generated `*_properties.rs` file. Also check each property's enum
  values and whether `feature-state` and `global-state` work.
- **Read, but not as the spec says**: compare the code with the spec's `doc`,
  `units`, `requires`, defaults and enum values, and with the code that draws it
  in GL JS (`src/render/draw_*.ts`, `src/data/bucket/`, `src/shaders/`) and
  MapLibre Native (`src/mln/renderer/`, `shaders/`). This is where unported
  tests hide gaps.
- **Upstream tests mln-rs lacks**: list GL JS's
  `test/integration/render/tests/`, MapLibre Native's
  `metrics/integration/{render,query,expression}-tests/` and
  maplibre-style-spec's `test/integration/expression/tests/` (e.g.
  `gh api repos/maplibre/maplibre-gl-js/git/trees/main?recursive=1`), and
  compare them with ours. Missing tests for a feature mln-rs claims to support
  are the best lead: port them and run them.
- **Ignored tests**: each one is a gap or a disagreement with the expected
  image. Check that its reason still holds.
- **`sdk-support`** in `v8.json`: an issue link instead of a version means that
  SDK doesn't draw the feature yet, so its spec may still change.
