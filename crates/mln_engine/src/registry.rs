//! The registrations that a runtime, its renderers and its tile workers
//! share: the expression functions, the layer types, the source types and
//! the extension members.
//!
//! An app fills a [`RegistryBuilder`] by explicit calls, `mln_builtins`'s
//! `register_builtins` first, and builds the [`Registry`] once. Tile workers
//! in other wasm instances make the same calls, since they run the app's own
//! module.

use std::sync::Arc;

use mln_style::collections::VecMap;
use mln_style::expression::Functions;
use mln_style::expression::functions::RegisterError;
use mln_style::layer::{ErasedStyleLayerType, StyleLayerType};
use mln_style::member::{ErasedMember, Member, Place};
use mln_style::source::StyleSourceType;
use mln_style::{Source, StyleRegistry};

use crate::layer::layer_type::{ErasedLayerType, entry};
use crate::layer::render_layer::Layer;
use crate::layer::tile_input::input::Find;
use crate::placement::{ErasedPlacement, Placement, participant};
use crate::source::erased::{self, ErasedSourceType};
use crate::source::{SourceSummary, SourceType, kind};

/// Makes a renderer's participant of a placing layer type.
type MakePlacement = fn() -> Box<dyn ErasedPlacement>;

/// What a [`Registry`] is built from.
#[derive(Default)]
pub struct RegistryBuilder {
    functions: Functions,
    layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
    placing_types: VecMap<&'static str, MakePlacement>,
    source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
    members: Vec<&'static dyn ErasedMember>,
}

impl std::fmt::Debug for RegistryBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryBuilder")
            .field("functions", &self.functions)
            .field("layer_types", &self.layer_types.keys().collect::<Vec<_>>())
            .field("placing_types", &self.placing_types.keys().collect::<Vec<_>>())
            .field("source_types", &self.source_types.keys().collect::<Vec<_>>())
            .field("members", &self.members.iter().map(|member| member.name()).collect::<Vec<_>>())
            .finish()
    }
}

impl RegistryBuilder {
    #[must_use]
    pub fn new() -> Self {
        RegistryBuilder::default()
    }

    pub fn functions_mut(&mut self) -> &mut Functions {
        &mut self.functions
    }

    /// Adds the layer type `L`, both its halves, under its style half's
    /// name. Fails for a name that an earlier type took. A type whose input
    /// is of another kind of source than its style half names doesn't
    /// compile.
    pub fn add_layer_type<L: Layer>(&mut self) -> Result<(), RegisterError> {
        const {
            assert!(
                L::Style::INFO.source as u8 == <L::Input as Find<L::Style>>::KIND as u8,
                "a layer type draws from the kind of source its style half names"
            );
        }
        let name = L::Style::INFO.name;
        if self.layer_types.contains_key(name) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        self.layer_types.insert(name, entry::<L>());
        Ok(())
    }

    /// Adds the layer type of `P`, a placement participant, both its
    /// halves, whose layers place with `P` ([`Placement`]). Fails for a name
    /// that an earlier type took.
    pub fn add_placing_layer_type<P: Placement>(&mut self) -> Result<(), RegisterError> {
        const { assert!(<P::Layer as Layer>::PLACES, "a placing layer type says that it places") }
        self.add_layer_type::<P::Layer>()?;
        self.placing_types.insert(<P::Layer as Layer>::Style::INFO.name, participant::<P>);
        Ok(())
    }

    /// Adds the source type `T`, both its halves. Fails for a name that an
    /// earlier type took. A type whose halves name different kinds of data
    /// doesn't compile.
    pub fn add_source_type<T: SourceType>(&mut self) -> Result<(), RegisterError> {
        const {
            assert!(
                T::Style::INFO.layer_kind as u8 == <T::Data as kind::Deliver>::KIND as u8,
                "a source type's halves name the same kind of data"
            );
        }
        let name = T::Style::INFO.name;
        if self.source_types.contains_key(name) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        self.source_types.insert(name, erased::entry::<T>());
        Ok(())
    }

    /// Adds the extension member `member`, which gives the style's members
    /// of its place and name a type. Fails for a place and name that an
    /// earlier member took.
    pub fn add_member<T: Send + Sync + 'static>(&mut self, member: &'static Member<T>) -> Result<(), RegisterError> {
        let taken = |added: &&dyn ErasedMember| added.place() == member.place() && added.name() == member.name();
        if self.members.iter().any(taken) {
            let place = match member.place() {
                Place::Root => "root",
                Place::Layer => "layer",
            };
            return Err(RegisterError::Duplicate(format!("{place} member {}", member.name())));
        }
        self.members.push(member);
        Ok(())
    }

    #[must_use]
    pub fn build(self) -> Arc<Registry> {
        let RegistryBuilder { functions, layer_types, placing_types, source_types, members } = self;
        Registry::new(Arc::new(functions), layer_types, placing_types, source_types, members)
    }
}

fn style_half(layer_type: &'static dyn ErasedLayerType) -> &'static dyn ErasedStyleLayerType {
    layer_type.style()
}

/// The expression functions, layer types, source types and extension
/// members that a style parses against, its renderer draws and its tile
/// workers lay out, built by a [`RegistryBuilder`]. It doesn't change once built.
///
/// A map and the renderer and tile workers that draw it must be given
/// registries filled by the same calls: they identify functions, layer
/// types and source types by name.
pub struct Registry {
    style: Arc<StyleRegistry>,
    layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
    placing_types: VecMap<&'static str, MakePlacement>,
    source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("style", &self.style).finish_non_exhaustive()
    }
}

impl Registry {
    fn new(
        functions: Arc<Functions>,
        layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
        placing_types: VecMap<&'static str, MakePlacement>,
        source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
        members: Vec<&'static dyn ErasedMember>,
    ) -> Arc<Registry> {
        let style_halves = layer_types.values().map(|&layer_type| style_half(layer_type));
        let style_source_halves = source_types.values().map(|&source_type| source_type.style());
        let style = StyleRegistry::new(functions, style_halves, style_source_halves, members);
        Arc::new(Registry { style: Arc::new(style), layer_types, placing_types, source_types })
    }

    #[must_use]
    pub fn style(&self) -> &Arc<StyleRegistry> {
        &self.style
    }

    #[must_use]
    pub fn functions(&self) -> &Arc<Functions> {
        self.style.functions()
    }

    #[must_use]
    pub fn layer_type(&self, name: &str) -> Option<&'static dyn ErasedLayerType> {
        self.layer_types.get(name).copied()
    }

    /// The makers of the participants of the placing layer types, in the
    /// order they registered.
    pub(crate) fn placing_types(&self) -> impl Iterator<Item = MakePlacement> + '_ {
        self.placing_types.values().copied()
    }

    pub(crate) fn source_type(&self, name: &str) -> Option<&'static dyn ErasedSourceType> {
        self.source_types.get(name).copied()
    }

    /// The attribution of `source`, as its type gives it: `None` for a type
    /// the registry lacks.
    #[must_use]
    pub fn source_attribution(&self, source: &Source) -> Option<String> {
        self.source_type(source.info().name)?.attribution(source)
    }

    /// What `source` reports about itself, as its type says: nothing for a
    /// type the registry lacks.
    #[must_use]
    pub fn source_summary(&self, source: &Source) -> SourceSummary {
        self.source_type(source.info().name).map(|source_type| source_type.summary(source)).unwrap_or_default()
    }
}

/// A context in which the values that the worker codec reads resolve the
/// registrations they name against this registry: layers' evaluated values
/// their layer type, and as for a [`StyleRegistry`], layers' style data and
/// compound expressions.
impl mln_wire::Context for &Registry {
    fn provide<'a>(&'a mut self, request: &mut mln_wire::Request<'a>) {
        let registry: &'a Registry = self;
        request.give(registry).give::<StyleRegistry>(registry.style()).give::<Functions>(registry.functions());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layer_type_registers_once() {
        type Circles = crate::tests::types::layers::TestFeatures<mln_style::layers::Circle>;
        let mut builder = RegistryBuilder::new();
        builder.add_layer_type::<Circles>().unwrap();
        assert_eq!(builder.add_layer_type::<Circles>(), Err(RegisterError::Duplicate("circle".to_owned())));
    }
}
