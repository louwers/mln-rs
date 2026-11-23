# mln-rs-ngon

The `org.maplibre.ngon` layer type for
[mln-rs](https://crates.io/crates/mln-rs): a layer of a geometry source that
draws a regular polygon, of 3 to 360 corners, at each of its points. It has the
paint properties of a circle layer, named `ngon-*`, and two more: `ngon-corners`
and `ngon-rotate`. Its polygons may collide with labels and with each other.

A caller adds it to the registry it starts its runtime with
(`mln_rs::Runtime::new`):

```rust
use std::sync::Arc;

use mln_rs::mln_render::{RegisterError, Registry, RegistryBuilder};

fn registry() -> Result<Arc<Registry>, RegisterError> {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder)?;
    mln_ngon::register(&mut builder)?;
    Ok(builder.build())
}
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
with the viewport keeps a corner up on the screen. A rendered-feature query hits
the polygon as drawn, stroke included.

## Collision

Two layout properties, both `true` by default, say how a layer's polygons
collide: with `ngon-allow-overlap` false, a polygon shows only where it overlaps
nothing placed before it; with `ngon-ignore-placement` false, what places after
it may not overlap it. They mean what a symbol layer's `icon-allow-overlap` and
`icon-ignore-placement` do, and so place among labels:

```json
{
  "id": "stops",
  "type": "org.maplibre.ngon",
  "source": "stops",
  "layout": { "ngon-allow-overlap": false, "ngon-ignore-placement": false },
  "paint": { "ngon-radius": 8, "ngon-color": "#2a9d8f" }
}
```

A layer that leaves both `true` doesn't place: its polygons show as its tiles
do, and a query hits each as drawn. Otherwise:

- What places first wins: the layers from the top of the style down, labels and
  polygons alike, and within a layer the tiles of the deeper zooms first, then
  from the top of the screen, and a tile's points in their features' order.
- A polygon collides as the circle through its stroke's corners. Its size is the
  one drawn at the camera's zoom, but where a feature sets its radius, stroke
  width or corners, each polygon collides at the size that its tile's zoom gives
  it, without its feature state.
- A polygon that shows fades in, and one that a placement hides fades out, over
  the style's transition, but a tile's polygons show or hide at once the first
  time a placement places them, so a polygon that a new tile hides disappears
  rather than fading. Until then, they all show. Those of a tile that the map no
  longer needs fade out.
- A rendered-feature query hits a polygon that places where it touches its
  circle's bounding box, and doesn't hit one that a placement hides.
- In tile mode, the polygons whose circle crosses their tile's border place
  first, so the tiles on either side of it agree on them as far as labels do.
- Layers that share their layout, which differ only in paint, show what the
  first of them placed.

It is written against the mln-rs facade alone, as a layer type outside mln-rs
is, and is the example that the layer contract's documentation points at.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
