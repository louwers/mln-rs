# mln-rs-ngon

The `org.maplibre.ngon` layer type for
[mln-rs](https://crates.io/crates/mln-rs): a layer of a geometry source that
draws a regular polygon, of 3 to 360 corners, at each of its points. It has a
circle layer's paint properties, named `ngon-*`, and two more: `ngon-corners`
and `ngon-rotate`.

An app adds it to the registry it starts its runtime with
(`mln_rs::Runtime::with_registry`):

```rust,ignore
let mut builder = mln_rs::mln_engine::RegistryBuilder::new();
mln_rs::mln_builtins::register_builtins(&mut builder)?;
mln_ngon::register(&mut builder)?;
let registry = builder.build();
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

## Properties

| property                                                        | default           |                                                           |
| --------------------------------------------------------------- | ----------------- | --------------------------------------------------------- |
| `ngon-radius`                                                   | 5                 | pixels from the center to a corner                        |
| `ngon-corners`                                                  | 5                 | rounded, from 3 to 360                                    |
| `ngon-rotate`                                                   | 0                 | degrees clockwise; 0 puts a corner on top                 |
| `ngon-color`, `ngon-opacity`                                    | black, 1          | the fill                                                  |
| `ngon-blur`                                                     | 0                 | the fraction of the polygon, from its edge in, that fades |
| `ngon-stroke-width`, `ngon-stroke-color`, `ngon-stroke-opacity` | 0, black, 1       | a stroke outside the fill, as wide along each edge        |
| `ngon-translate`, `ngon-translate-anchor`                       | `[0, 0]`, `map`   |                                                           |
| `ngon-pitch-alignment`, `ngon-pitch-scale`                      | `viewport`, `map` |                                                           |

The first nine are data-driven and may read feature state; the last four are the
same for every feature, as a circle layer's are, and mean what theirs do. A
polygon aligned with the map lies in its plane and turns with it; one aligned
with the viewport keeps its first corner up on the screen. Over terrain, each
polygon stands on the ground under its center. A rendered-feature query hits the
polygon as drawn, stroke included.

Its code is under the root license ([LICENSE.md](LICENSE.md)).

Part of [mln-rs](https://github.com/maplibre/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
