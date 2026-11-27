//! What a crate needs to write a layer type: the engine's layer contract
//! ([`Layer`] and its contexts, inputs, layouts and buckets), the style half
//! ([`StyleLayerType`], its properties declared with [`layer_properties!`]
//! and the extension members it reads, [`Member`]), the registration
//! ([`RegistryBuilder::add_layer_type`]), and what a layer draws with
//! ([`draw`], [`shaders`]), places with ([`placement`]) and routes other
//! layers' draws through ([`Router`]).
//!
//! `mln_engine::layer` describes the contract. The values that its
//! signatures carry are those of the crates they come from: the camera and
//! tiles in [`mln_engine`], the style's types and
//! expressions in [`mln_style`], and the graphics enums in
//! [`mln_gfx`].
//!
//! `examples/custom-layer` writes a layer type with these and registers
//! it beside another crate's.

/// What a layer draws: the frame it draws for, its meshes and their
/// vertices, indexes, textures and targets, and the draws it pushes.
pub use mln_engine::draw;
pub use mln_engine::draw::{Draws, LayerFrame, Mesh, Resources};
pub use mln_engine::layer::*;
// The engine hides them from its own documentation; they're the contract
// of shared targets.
pub use mln_engine::layer::{Router, Routes};
/// The symbol placement that a placing layer type takes part in.
pub use mln_engine::placement;
pub use mln_engine::placement::Placement;
pub use mln_engine::{Registry, RegistryBuilder, paint_property_binders};
pub use mln_style::expression::functions::RegisterError;
pub use mln_style::layer::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
pub use mln_style::layer_properties;
pub use mln_style::member::Member;
pub use mln_style::property::table::{PropertyTable, PropertyVisitor};

/// A layer type's programs: their attributes, blocks, textures and text in
/// each shading language, and the projection variants they're built for.
pub mod shaders {
    pub use mln_gfx::projection_variant::ProjectionVariant;
    pub use mln_gfx::shaders::layer_ubo::ProjectionUBO;
    pub use mln_gfx::shaders::program::{
        Attribute, Program, ProgramBuffer, ProgramTexture, Projection, Property, Stages,
    };
    pub use mln_gfx::shaders::source::{GlslEsDeclarations, GlslEsSource, GlslSource, MslSource, Sources, WgslSource};
    pub use mln_gfx::uniform_block;
}
