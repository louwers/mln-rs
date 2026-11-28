//! The registrations that a runtime, its renderers and its tile workers
//! share: the expression functions, the layer types, the source types, the
//! extension members, the routers of layers into shared targets and the
//! projections.
//!
//! An app fills a [`RegistryBuilder`] by explicit calls, `mln_builtins`'s
//! `register_builtins` first, and builds the [`Registry`] once. Tile workers
//! in other wasm instances make the same calls, since they run the app's own
//! module.

use std::sync::Arc;

use mln_style::collections::VecMap;
use mln_style::crs::{Crs, TileMatrixSet};
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
use crate::render::shared_targets::Router;
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
    routers: Vec<&'static dyn Router>,
    projections: VecMap<String, Arc<Crs>>,
}

impl std::fmt::Debug for RegistryBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryBuilder")
            .field("functions", &self.functions)
            .field("layer_types", &self.layer_types.keys().collect::<Vec<_>>())
            .field("placing_types", &self.placing_types.keys().collect::<Vec<_>>())
            .field("source_types", &self.source_types.keys().collect::<Vec<_>>())
            .field("members", &self.members.iter().map(|member| member.name()).collect::<Vec<_>>())
            .field("routers", &self.routers.len())
            .field("projections", &self.projections.keys().collect::<Vec<_>>())
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

    /// Adds `router`, which declares the shared targets of the frames of
    /// each version of a style and routes its layers' draws into them. The
    /// routers route in the order they were added: of two routes of a
    /// layer, the later stands.
    pub fn add_router(&mut self, router: &'static dyn Router) {
        self.routers.push(router);
    }

    /// Adds the projection `crs`, which a style's `projection.type` names.
    /// Fails for a name without a colon, which built-in projections keep, a
    /// name that an earlier projection took, and a tile matrix set whose
    /// origin isn't finite or whose extent isn't positive and finite.
    pub fn add_projection(&mut self, crs: Crs) -> Result<(), RegisterError> {
        if !crs.name.contains(':') {
            return Err(RegisterError::ProjectionName(crs.name));
        }
        self.insert_projection(crs)
    }

    /// Adds `crs`, of any name. Fails as [`add_projection`](Self::add_projection)
    /// does for a taken name or a tile matrix set.
    fn insert_projection(&mut self, crs: Crs) -> Result<(), RegisterError> {
        if self.projections.contains_key(crs.name.as_str()) {
            return Err(RegisterError::Duplicate(crs.name));
        }
        let TileMatrixSet { origin, extent_at_zoom0: extent } = crs.tile_matrix_set;
        let valid = origin.iter().all(|value| value.is_finite()) && extent.is_finite() && extent > 0.0;
        if !valid {
            return Err(RegisterError::TileMatrixSet(crs.name));
        }
        self.projections.insert(crs.name.clone(), Arc::new(crs));
        Ok(())
    }

    #[must_use]
    pub fn build(self) -> Arc<Registry> {
        let RegistryBuilder { functions, layer_types, placing_types, source_types, members, routers, projections } =
            self;
        let projections = projections.into_iter().map(|(_, crs)| crs);
        Registry::new(Arc::new(functions), layer_types, placing_types, source_types, members, routers, projections)
    }
}

/// Adds the built-in projections, whose names have no colon: `identity`
/// ([`Crs::identity`]). Fails when an earlier call added them.
pub fn register_builtin_projections(builder: &mut RegistryBuilder) -> Result<(), RegisterError> {
    builder.insert_projection(Crs::identity())
}

fn style_half(layer_type: &'static dyn ErasedLayerType) -> &'static dyn ErasedStyleLayerType {
    layer_type.style()
}

/// The expression functions, layer types, source types and extension
/// members that a style parses against, its renderer draws and its tile
/// workers lay out, and the projections that the style's `projection.type`
/// names, built by a [`RegistryBuilder`]. It doesn't change once built.
///
/// A map and the renderer and tile workers that draw it must be given
/// registries filled by the same calls: they identify functions, layer
/// types and source types by name.
pub struct Registry {
    style: Arc<StyleRegistry>,
    layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
    placing_types: VecMap<&'static str, MakePlacement>,
    source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
    routers: Vec<&'static dyn Router>,
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
        routers: Vec<&'static dyn Router>,
        projections: impl IntoIterator<Item = Arc<Crs>>,
    ) -> Arc<Registry> {
        let style_halves = layer_types.values().map(|&layer_type| style_half(layer_type));
        let style_source_halves = source_types.values().map(|&source_type| source_type.style());
        let style = StyleRegistry::new(functions, style_halves, style_source_halves, members, projections);
        Arc::new(Registry { style: Arc::new(style), layer_types, placing_types, source_types, routers })
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

    /// The routers, in the order they were added.
    pub(crate) fn routers(&self) -> &[&'static dyn Router] {
        &self.routers
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

    /// The identity CRS under the name `name`.
    fn identity_named(name: &str) -> Crs {
        let Crs { converter, tile_matrix_set, .. } = Crs::identity();
        Crs { name: name.to_owned(), converter, tile_matrix_set }
    }

    // GL JS: addProjection registers a CRS under its name.
    #[test]
    fn a_projection_registers_under_its_name() {
        let mut builder = RegistryBuilder::new();
        builder.add_projection(identity_named("test:crs")).unwrap();
        let registry = builder.build();
        let mapping = registry.style().projection("test:crs").expect("registered");
        assert_eq!(mapping.crs().name, "test:crs");
        assert!(registry.style().projection("test:other").is_none());
    }

    // GL JS: addProjection throws when the name is already registered.
    #[test]
    fn a_projection_name_registers_once() {
        let mut builder = RegistryBuilder::new();
        builder.add_projection(identity_named("test:crs")).unwrap();
        assert_eq!(
            builder.add_projection(identity_named("test:crs")),
            Err(RegisterError::Duplicate("test:crs".to_owned()))
        );
    }

    // GL JS: addProjection throws for a name without a colon, built in or
    // not, and registers nothing.
    #[test]
    fn a_projection_name_needs_a_colon() {
        let mut builder = RegistryBuilder::new();
        let names = ["mercator", "globe", "vertical-perspective", "identity", "equal-earth"];
        for name in names {
            assert_eq!(
                builder.add_projection(identity_named(name)),
                Err(RegisterError::ProjectionName(name.to_owned()))
            );
        }
        let registry = builder.build();
        for name in names {
            assert!(registry.style().projection(name).is_none(), "{name}");
        }
    }

    // GL JS: addProjection throws for a zoom 0 extent that isn't positive.
    #[test]
    fn a_tile_matrix_set_needs_a_finite_origin_and_a_positive_extent() {
        let mut builder = RegistryBuilder::new();
        let tile_matrix_sets = [
            TileMatrixSet { origin: [0.0, 0.0], extent_at_zoom0: 0.0 },
            TileMatrixSet { origin: [0.0, 0.0], extent_at_zoom0: -1.0 },
            TileMatrixSet { origin: [0.0, 0.0], extent_at_zoom0: f64::NAN },
            TileMatrixSet { origin: [0.0, 0.0], extent_at_zoom0: f64::INFINITY },
            TileMatrixSet { origin: [f64::NAN, 0.0], extent_at_zoom0: 1.0 },
            TileMatrixSet { origin: [0.0, f64::NEG_INFINITY], extent_at_zoom0: 1.0 },
        ];
        for tile_matrix_set in tile_matrix_sets {
            let crs = Crs { tile_matrix_set, ..identity_named("test:crs") };
            assert_eq!(builder.add_projection(crs), Err(RegisterError::TileMatrixSet("test:crs".to_owned())));
        }
        assert!(builder.build().style().projection("test:crs").is_none());
    }

    // GL JS: getRegisteredProjection finds the pre-registered identity
    // projection.
    #[test]
    fn the_builtin_projections_are_identity() {
        let mut builder = RegistryBuilder::new();
        register_builtin_projections(&mut builder).unwrap();
        assert_eq!(register_builtin_projections(&mut builder), Err(RegisterError::Duplicate("identity".to_owned())));
        let registry = builder.build();
        let identity = registry.style().projection("identity").expect("identity").crs();
        assert_eq!(identity.tile_matrix_set, TileMatrixSet { origin: [-90.0, 90.0], extent_at_zoom0: 180.0 });
        let mut positions = [[0.0; 2]; 2];
        identity.converter.forward(&[[12.5, -41.9], [-180.0, 90.0]], &mut positions);
        assert_eq!(positions, [[12.5, -41.9], [-180.0, 90.0]]);
        let mut lng_lats = [[0.0; 2]; 1];
        identity.converter.inverse(&[[12.5, -41.9]], &mut lng_lats);
        assert_eq!(lng_lats, [[12.5, -41.9]]);
        assert!(RegistryBuilder::new().build().style().projection("identity").is_none());
    }
}
