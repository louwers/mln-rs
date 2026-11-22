# mln-rs-image-process

Named render targets for [mln-rs](https://crates.io/crates/mln-rs) styles, and
the `org.maplibre.image-process` layer type that processes them: a style
declares targets at its root, draws layers of any type into one instead of the
map, and an image-process layer reads a target through blurs, thresholds, colors
and combinations with other targets, and draws the result over the map in its
place.

A caller adds the keys and the layer type to the registry it starts its runtime
with:

```rust
let mut builder = mln_rs::mln_render::RegistryBuilder::new();
mln_rs::register_builtins(&mut builder)?;
mln_image_process::register(&mut builder)?;
```

and a style draws a glow under its roads:

```json
{
  "org.maplibre:targets": [{ "id": "roads" }],
  "layers": [
    {
      "id": "roads-glow-source",
      "type": "line",
      "source": "roads",
      "org.maplibre:destination-target": "roads",
      "paint": { "line-width": 6 }
    },
    {
      "id": "roads-glow",
      "type": "org.maplibre.image-process",
      "org.maplibre:source-target": "roads",
      "org.maplibre:operations": [
        { "type": "blur", "size": 5 },
        { "type": "color", "value": "#ffd040" }
      ]
    }
  ]
}
```

It is written against the mln-rs facade alone, as an extension outside mln-rs
is.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
