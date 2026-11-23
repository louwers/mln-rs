#![doc = include_str!("../README.md")]
//!
//! # The crate's parts
//!
//! - the style keys and the values they parse to (`keys.rs`): [`SOURCE`]
//!   and [`OPERATIONS`];
//! - the style half, here: [`ImageProcess`], and [`register`];
//! - the passes that an image-process layer's operations make (`plan.rs`);
//! - the program, with its block and its shaders in the four shading
//!   languages (`program.rs`, `wgsl.rs`, `msl.rs`, `glsl.rs`,
//!   `glsl_es.rs`), which samples a pass's input and applies its operations;
//! - the render half, [`ImageProcessLayer`], which draws each pass but the
//!   last into a scratch target of its own (`layer.rs`).

mod glsl;
mod glsl_es;
mod keys;
mod layer;
mod msl;
mod plan;
mod program;
mod wgsl;

pub use keys::{Combine, OPERATIONS, Operation, Operations, SOURCE, SourceTarget};
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
    builder.add_style_key(&SOURCE)?;
    builder.add_style_key(&OPERATIONS)?;
    builder.add_layer_type::<ImageProcessLayer>()
}
