# mln-rs-location-puck

The `org.maplibre.location-puck` layer type for
[mln-rs](https://crates.io/crates/mln-rs): a layer without a source that draws a
device's location at one geographic position, a puck with a border, an arrow and
a sector for its heading, a circle for its accuracy in meters, and a shadow.
Every shape is drawn in its fragment shader, so it needs no images and stays
crisp at any pixel ratio.

A caller adds it to the registry it starts its runtime with:

```rust
let mut builder = mln_rs::mln_render::RegistryBuilder::new();
mln_rs::register_builtins(&mut builder)?;
mln_location_puck::register(&mut builder)?;
```

and an app adds the layer, then sets its paint properties as fixes come in:

```json
{
  "id": "puck",
  "type": "org.maplibre.location-puck",
  "paint": {
    "position": [-122.4194, 37.7749],
    "bearing": 35,
    "bearing-visible": true,
    "accuracy-radius": 80
  }
}
```

The properties transition, the bearing the shorter way around, and a
rendered-feature query that hits the puck or its arrow returns a point at the
position.

It is written against the mln-rs facade alone, as a layer type outside mln-rs
is.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
