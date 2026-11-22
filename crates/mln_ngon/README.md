# mln-rs-ngon

The `org.maplibre.ngon` layer type for
[mln-rs](https://crates.io/crates/mln-rs): a layer of a geometry source that
draws a regular polygon, of 3 to 360 corners, at each of its points. It has the
paint properties of a circle layer, named `ngon-*`, and two more: `ngon-corners`
and `ngon-rotate`.

A caller adds it to the registry it starts its runtime with:

```rust
let mut builder = mln_rs::mln_render::RegistryBuilder::new();
mln_rs::register_builtins(&mut builder)?;
mln_ngon::register(&mut builder)?;
```

and a style draws it:

```json
{
  "id": "stops",
  "type": "org.maplibre.ngon",
  "source": "stops",
  "paint": {
    "ngon-radius": 8,
    "ngon-corners": ["get", "sides"],
    "ngon-color": "#2a9d8f",
    "ngon-stroke-width": 2,
    "ngon-stroke-color": "#1d3557"
  }
}
```

It is written against the mln-rs facade alone, as a layer type outside mln-rs
is, and is the example that the layer contract's documentation points at.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
