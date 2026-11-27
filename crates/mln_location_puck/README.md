# mln-rs-location-puck

The `org.maplibre.location-puck` layer type for
[mln-rs](https://crates.io/crates/mln-rs): a layer without a source that draws a
device's location at one geographic position, a puck with a border, an arrow and
a sector for its heading, a circle for its accuracy in meters, and a shadow.
Every shape is drawn in its fragment shader, so it needs no images and stays
crisp at any pixel ratio.

An app adds it to the registry it starts its runtime with
(`mln_rs::Runtime::with_registry`):

```rust,ignore
let mut builder = mln_rs::mln_engine::RegistryBuilder::new();
mln_rs::mln_builtins::register_builtins(&mut builder)?;
mln_location_puck::register(&mut builder)?;
let registry = builder.build();
```

then adds the layer and sets its paint properties as fixes come in:

```json
{
  "id": "puck",
  "type": "org.maplibre.location-puck",
  "paint": {
    "puck-position": [-122.4194, 37.7749],
    "puck-bearing": 35,
    "puck-bearing-visible": true,
    "puck-accuracy-radius": 80
  }
}
```

## Properties

Each is a paint property, the same for the whole layer, and transitions: the
bearing turns, and the position moves, the shorter way around, across the
antimeridian where that is shorter. Sizes are logical pixels but for
`puck-accuracy-radius`. A size of 0 hides its part. The parts draw from the
bottom up: the accuracy circle, the bearing's sector, the shadow, the arrow and
the puck.

| property                                                      | default              |                                                                                                  |
| ------------------------------------------------------------- | -------------------- | ------------------------------------------------------------------------------------------------ |
| `puck-position`                                               | `[0, 0]`             | `[longitude, latitude]`                                                                          |
| `puck-bearing`                                                | 0                    | the heading, degrees clockwise from north                                                        |
| `puck-bearing-visible`                                        | `false`              | whether the arrow and the sector show                                                            |
| `puck-bearing-radius`, `puck-bearing-arrow-color`             | 18, white            | the arrow                                                                                        |
| `puck-bearing-accuracy`                                       | 0                    | the sector's half width, degrees                                                                 |
| `puck-bearing-accuracy-radius`, `puck-bearing-accuracy-color` | 64, translucent blue | the sector, which fades outward                                                                  |
| `puck-accuracy-radius`                                        | 0                    | the accuracy circle's radius, meters                                                             |
| `puck-accuracy-color`                                         | translucent blue     | its fill                                                                                         |
| `puck-accuracy-border-width`, `puck-accuracy-border-color`    | 0, translucent blue  | its border                                                                                       |
| `puck-radius`, `puck-color`                                   | 8, blue              | the puck's fill                                                                                  |
| `puck-border-width`, `puck-border-color`                      | 2, white             | its border, outside the fill                                                                     |
| `puck-shadow-radius`, `puck-shadow-color`                     | 0, translucent black | a soft shadow                                                                                    |
| `puck-perspective-compensation`                               | 0.85                 | from 0, the puck shrinks with distance as the map does, to 1, it keeps its size                  |
| `puck-tilt-displacement`                                      | 0                    | under pitch, how far the puck and arrow lift up the screen and the shadow drops, pixels a radian |

Every part lies on the ground, so pitch foreshortens it, and the puck, the arrow
and the shadow turn with the bearing. It draws in every world copy, on Mercator
and on the globe. Over terrain, it stands on the ground under the position and
hides behind the ground in front of it. A rendered-feature query that hits the
puck or the arrow where they show returns a point at the position.

Its code is under the root license ([LICENSE.md](LICENSE.md)).

Part of [mln-rs](https://github.com/maplibre/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
