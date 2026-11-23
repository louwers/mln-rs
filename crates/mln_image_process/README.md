# mln-rs-image-process

The `org.maplibre.image-process` layer type for
[mln-rs](https://crates.io/crates/mln-rs) styles, which processes named render
targets: a style declares targets at its root and draws layers of any type into
one instead of the map, and an image-process layer reads a target through blurs,
thresholds, colors and combinations with other targets, and draws the result
over the map in its place, or into a target of its own.

A caller adds the layer type and its keys to the registry it starts its runtime
with (`mln_rs::Runtime::new`):

```rust
use std::sync::Arc;

use mln_rs::mln_render::{RegisterError, Registry, RegistryBuilder};

fn registry() -> Result<Arc<Registry>, RegisterError> {
    let mut builder = RegistryBuilder::new();
    mln_rs::register_builtins(&mut builder)?;
    mln_image_process::register(&mut builder)?;
    Ok(builder.build())
}
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

## Keys

| key                               | where                  | value                                                                                                                          |
| --------------------------------- | ---------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `org.maplibre:targets`            | the style's root       | `[{"id": <name>, "precision": "standard" \| "high"}, ...]`: targets as large as the map, of 8 bits a channel or of half floats |
| `org.maplibre:destination-target` | any layer              | the target it draws into instead of the map                                                                                    |
| `org.maplibre:source-target`      | an image-process layer | the target it reads                                                                                                            |
| `org.maplibre:operations`         | an image-process layer | what it does to what it reads, in order                                                                                        |

Every registry holds the first two keys, of the engine's named targets; this
crate adds the last two.

A target holds what the layers drawn into it draw in the frame, wherever they
are among the layers, and nothing that the map doesn't. A layer of 3D geometry,
and one whose route would make a target read what it draws, draws on the map
instead, with a warning. A rendered-feature query finds a layer drawn into a
target as if it drew on the map, even where an operation erased what it drew:
give the query the layers it means.

## Operations

Each is an object of its `type` and its parameters, which may follow the zoom.
They act on premultiplied colors, whose alpha is their coverage, and their sizes
are logical pixels. What lies past the map's edges is what is at them.

| type        | parameters                           |                                                                                                                                                                                                                                |
| ----------- | ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `blur`      | `size`, 0 to 64                      | a Gaussian blur, `size` its standard deviation                                                                                                                                                                                 |
| `threshold` | `cutoff`, 0 to 255, default 128      | coverage at or above the cutoff made whole and the rest none, antialiased                                                                                                                                                      |
| `color`     | `value`                              | every pixel painted the color, its coverage kept                                                                                                                                                                               |
| `opacity`   | `value`, 0 to 1                      | coverage scaled                                                                                                                                                                                                                |
| `combine`   | `target`, `mode`, default `multiply` | the image with another target's: `multiply` keeps it where the other covers, `subtract` where it doesn't, `max` and `min` take the more and the less covered of the two, `over` draws the other over it and `under` beneath it |

Without operations, a layer draws its source target as it is.

A registry without this crate loads a style that uses it with a warning for each
image-process layer, which it leaves out, and draws the layers routed into
targets into them, where nothing reads them.

It is written against the mln-rs facade alone, as an extension outside mln-rs
is: the engine draws layers into the targets and orders the frame's passes by
what reads what, and this crate's layer type reads them.

Part of [mln-rs](https://github.com/louwers/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
