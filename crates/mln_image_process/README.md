# mln-rs-image-process

The `org.maplibre.image-process` layer type for
[mln-rs](https://crates.io/crates/mln-rs), and the style members that feed it: a
style declares shared targets at its root and draws layers of any type into one
instead of the map, and an image-process layer reads a target through blurs,
thresholds, colors and combinations with other targets, and draws the result
over the map in its place, or into a target of its own.

An app adds it to the registry it starts its runtime with
(`mln_rs::Runtime::with_registry`):

```rust,ignore
let mut builder = mln_rs::mln_engine::RegistryBuilder::new();
mln_rs::mln_builtins::register_builtins(&mut builder)?;
mln_image_process::register(&mut builder)?;
let registry = builder.build();
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

## Members

| member                            | where                  | value                                                                                                                       |
| --------------------------------- | ---------------------- | --------------------------------------------------------------------------------------------------------------------------- |
| `org.maplibre:targets`            | the style's root       | `[{"id": <name>, "precision": "standard" \| "high"}, ...]`: targets as large as the map, of 8 bits a channel or half floats |
| `org.maplibre:destination-target` | any layer              | the target it draws into instead of the map                                                                                 |
| `org.maplibre:source-target`      | an image-process layer | the target it reads                                                                                                         |
| `org.maplibre:operations`         | an image-process layer | what it does to what it reads, in order                                                                                     |

A target holds what the layers drawn into it draw in the frame, wherever they
are among the layers. A layer of 3D geometry, one whose blend reads what is
behind it, and one whose route would make a target read what it draws draw on
the map instead, with a warning. Over terrain, a layer drawn into a target draws
flat, without draping. A `high` target is of 8 bits a channel where the device
can't draw into half floats.

## Operations

Each is an object of its `type` and its parameters, which may follow the zoom.
They act on premultiplied colors, whose alpha is their coverage, and their sizes
are logical pixels. What lies past the map's edges is what is at them.

| type        | parameters                           |                                                                                                                                                                                                                               |
| ----------- | ------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `blur`      | `size`, 0 to 64                      | a Gaussian blur, `size` its standard deviation                                                                                                                                                                                |
| `threshold` | `cutoff`, 0 to 255, default 128      | coverage above the cutoff made whole and below it none, antialiased over an edge centered on the cutoff                                                                                                                       |
| `color`     | `value`                              | every pixel painted the color, its coverage kept                                                                                                                                                                              |
| `opacity`   | `value`, 0 to 1                      | coverage scaled                                                                                                                                                                                                               |
| `combine`   | `target`, `mode`, default `multiply` | the image with another target's: `multiply` keeps it where the other covers, `subtract` where it doesn't, `max` and `min` take each channel's more and less of the two, `over` draws the other over it and `under` beneath it |

Without operations, a layer draws its source target as it is.

A registry without this crate loads a style that uses it with a warning for each
image-process layer, which it leaves out, and for each member, and draws the
layers routed into targets on the map.

Its code is under the root license ([LICENSE.md](LICENSE.md)).

Part of [mln-rs](https://github.com/maplibre/mln-rs), a Rust port of the
[MapLibre Native](https://github.com/maplibre/maplibre-native) map rendering
core. All its crates are released together, at the same version.
