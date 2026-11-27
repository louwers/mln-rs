//! The `org.maplibre.image-process` layer type, and the style members that
//! route other layers' draws into shared targets for it to read
//! ([README](https://github.com/maplibre/mln-rs/tree/main/crates/mln_image_process)).
//! [`register`] adds them to a registry.
//!
//! The members, the values they parse to and the router that turns them
//! into the frames' shared targets and routes are in `members.rs`. An
//! image-process layer plans its operations into passes (`plan.rs`) and
//! draws each pass but the last into a scratch target of its own
//! (`layer.rs`), with its program (`program.rs`, with its text in each
//! shading language).

#[rustfmt::skip]
mod glsl;
#[rustfmt::skip]
mod glsl_es;
mod layer;
mod members;
#[rustfmt::skip]
mod msl;
mod plan;
mod program;
mod wgsl;

use mln_engine::RegistryBuilder;
use mln_style::expression::functions::RegisterError;
use mln_style::layer::{LayerStyle, LayerTypeInfo, NoProperties, SourceKind, StyleLayerType};
use mln_style::property::table::{PropertyTable, PropertyVisitor};

use crate::layer::ImageProcessLayer;
use crate::members::{DESTINATION, OPERATIONS, SOURCE, TARGETS, TargetRouter};

/// The `org.maplibre.image-process` layer type's style half: a layer
/// without a source or properties, whose members say what it reads and
/// does.
pub(crate) struct ImageProcess;

static PROPERTIES: PropertyTable = PropertyTable::new(&[], &[]);

impl StyleLayerType for ImageProcess {
    const INFO: LayerTypeInfo =
        LayerTypeInfo { name: "org.maplibre.image-process", source: SourceKind::None, from_json: true };
    const PROPERTIES: &'static PropertyTable = &PROPERTIES;

    type LayoutProperties = NoProperties;
    type PaintProperties = NoProperties;
    type Extra = ();

    fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(_slot: usize, _visitor: V) -> Option<V::Output> {
        None
    }
}

static ROUTER: TargetRouter = TargetRouter;

/// Adds to `builder` the `org.maplibre.image-process` layer type, the style
/// members `org.maplibre:targets`, `org.maplibre:destination-target`,
/// `org.maplibre:source-target` and `org.maplibre:operations`, and the
/// router of the first two. Fails if the builder has a layer type or a
/// member of one of those names.
pub fn register(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.add_member(&TARGETS)?;
    builder.add_member(&DESTINATION)?;
    builder.add_member(&SOURCE)?;
    builder.add_member(&OPERATIONS)?;
    builder.add_layer_type::<ImageProcessLayer>()?;
    builder.add_router(&ROUTER);
    Ok(())
}
