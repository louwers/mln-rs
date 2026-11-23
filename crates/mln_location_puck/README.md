# mln-rs-location-puck

The `org.maplibre.location-puck` layer type for
[mln-rs](https://crates.io/crates/mln-rs): a layer without a source that draws a
device's location at one geographic position, a puck with a border, an arrow and
a sector for its heading, a circle for its accuracy in meters, and a shadow.
Every shape is drawn in its fragment shader, so it needs no images and stays
crisp at any pixel ratio.

A caller adds it to the registry it starts its runtime with
(`mln_rs::Runtime::new`):

```rust
use std::sync::Arc;

use mln_rs::mln_render::{RegisterError, Registry, RegistryBuilder};

fn registry() -> Result<Arc<Registry>, RegisterError> {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder)?;
    mln_location_puck::register(&mut builder)?;
    Ok(builder.build())
}
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

## Properties

Each is a paint property, the same for the whole layer, and transitions; the
bearing turns, and the position moves, the shorter way around, across the
antimeridian where that is shorter. Sizes are logical pixels but for
`accuracy-radius`. A size of 0 hides its part. The parts draw from the bottom
up: the accuracy circle, the bearing's sector, the shadow, the arrow and the
puck.

| property                                            | default              |                                                                                                  |
| --------------------------------------------------- | -------------------- | ------------------------------------------------------------------------------------------------ |
| `position`                                          | `[0, 0]`             | `[longitude, latitude]`                                                                          |
| `bearing`                                           | 0                    | the heading, degrees clockwise from north                                                        |
| `bearing-visible`                                   | `false`              | whether the arrow and the sector show                                                            |
| `bearing-radius`, `bearing-arrow-color`             | 18, white            | the arrow                                                                                        |
| `bearing-accuracy`                                  | 0                    | the sector's half width, degrees                                                                 |
| `bearing-accuracy-radius`, `bearing-accuracy-color` | 64, translucent blue | the sector, which fades outward                                                                  |
| `accuracy-radius`                                   | 0                    | the accuracy circle's radius, meters                                                             |
| `accuracy-color`                                    | translucent blue     | its fill                                                                                         |
| `accuracy-border-width`, `accuracy-border-color`    | 0, translucent blue  | its border                                                                                       |
| `puck-radius`, `puck-color`                         | 8, blue              | the puck's fill                                                                                  |
| `puck-border-width`, `puck-border-color`            | 2, white             | its border, outside the fill                                                                     |
| `shadow-radius`, `shadow-color`                     | 0, translucent black | a soft shadow                                                                                    |
| `perspective-compensation`                          | 0.85                 | from 0, the puck shrinks with distance as the map does, to 1, it keeps its size                  |
| `tilt-displacement`                                 | 0                    | under pitch, how far the puck and arrow lift up the screen and the shadow drops, pixels a radian |

Every part lies on the ground, so pitch foreshortens it, and the puck, the arrow
and the shadow turn with the bearing. It draws in every world copy, on Mercator
and on the globe. A rendered-feature query that hits the puck or the arrow
returns a point at the position.

It is written against the mln-rs facade alone, as a layer type outside mln-rs
is.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
