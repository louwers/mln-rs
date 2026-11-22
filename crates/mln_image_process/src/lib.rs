//! Named render targets for a style, and the `org.maplibre.image-process`
//! layer type that processes them: the style's root declares targets, a
//! layer of any type draws into one instead of the map, and an
//! image-process layer reads one through its operations and draws the
//! result over the map in its place, or into a target of its own. A caller
//! adds the keys and the layer type to its registry with [`register`].
//!
//! It is written against the `mln_rs` facade alone, as an extension outside
//! mln-rs is: the engine draws layers into the targets and orders the
//! frame's passes by what reads what, and this crate names the keys that
//! drive it and the layer type that reads them. Its modules:
//!
//! - the style keys and the values they parse to (`keys.rs`);
//! - the passes that an image-process layer's operations make (`plan.rs`);
//! - the program, with its block and its shaders in the four shading
//!   languages (`program.rs`, `wgsl.rs`, `msl.rs`, `glsl.rs`,
//!   `glsl_es.rs`), which samples a pass's input and applies its operations;
//! - the render half, [`ImageProcessLayer`], which draws each pass but the
//!   last into a scratch target of its own (`layer.rs`).
//!
//! # Keys
//!
//! | key | where | value |
//! |---|---|---|
//! | `org.maplibre:targets` | the style's root | `[{"id": <name>, "precision": "standard" \| "high"}, ...]`: targets as large as the map, of 8 bits a channel or of half floats |
//! | `org.maplibre:destination-target` | any layer | the target it draws into instead of the map |
//! | `org.maplibre:source-target` | an image-process layer | the target it reads |
//! | `org.maplibre:operations` | an image-process layer | what it does to what it reads, in order |
//!
//! A target holds what the layers drawn into it draw in the frame, wherever
//! they are among the layers, and nothing that the map doesn't. A layer of
//! 3D geometry, and one whose route would make a target read what it draws,
//! draws on the map instead, with a warning.
//!
//! # Operations
//!
//! Each is an object of its `type` and its parameters, which may follow the
//! zoom. They act on premultiplied colors, whose alpha is their coverage,
//! and their sizes are logical pixels. What lies past the map's edges is
//! what is at them.
//!
//! | type | parameters | |
//! |---|---|---|
//! | `blur` | `size`, 0 to 64 | a Gaussian blur, `size` its standard deviation |
//! | `threshold` | `cutoff`, 0 to 255, default 128 | coverage at or above the cutoff made whole and the rest none, antialiased |
//! | `color` | `value` | every pixel painted the color, its coverage kept |
//! | `opacity` | `value`, 0 to 1 | coverage scaled |
//! | `combine` | `target`, `mode`, default `multiply` | the image with another target's: `multiply` keeps it where the other covers, `subtract` where it doesn't, `max` and `min` take the more and the less covered of the two, `over` draws the other over it and `under` beneath it |
//!
//! Without operations, a layer draws its source target as it is.
//!
//! # Without this crate
//!
//! A registry without these keys loads a style that uses them with a
//! warning for each member it doesn't know and each image-process layer,
//! which it leaves out, and draws every other layer on the map.

mod glsl;
mod glsl_es;
mod keys;
mod layer;
mod msl;
mod plan;
mod program;
mod wgsl;

pub use keys::{Combine, DESTINATION, OPERATIONS, Operation, Operations, SOURCE, SourceTarget, TARGETS};
pub use layer::ImageProcessLayer;
use mln_rs::layer::{LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_rs::mln_render::{RegisterError, RegistryBuilder};

/// The `org.maplibre.image-process` layer type's style half: a layer
/// without a source or properties, whose keys say what it reads and does.
pub struct ImageProcess;

impl StyleLayerType for ImageProcess {
    const INFO: LayerTypeInfo =
        LayerTypeInfo { name: "org.maplibre.image-process", source: SourceKind::None, from_json: true };

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Data = ();
}

/// Adds the style keys and the `org.maplibre.image-process` layer type to
/// `builder`.
pub fn register(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_style_key(&TARGETS)?;
    builder.add_style_key(&DESTINATION)?;
    builder.add_style_key(&SOURCE)?;
    builder.add_style_key(&OPERATIONS)?;
    builder.add_layer_type::<ImageProcessLayer>()
}
