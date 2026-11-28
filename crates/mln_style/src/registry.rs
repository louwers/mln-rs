//! The registrations that a style parses against: its expression functions,
//! its layer types, its source types and its extension members, and the
//! projections that its `projection.type` can name.

use std::sync::Arc;

use crate::collections::VecMap;
use crate::conversion::{Convertible, Error};
use crate::crs::{Crs, CrsMapping};
use crate::expression::Functions;
use crate::feature::Value;
use crate::layer::{ErasedStyleLayerType, Layer, SourceKind};
use crate::log_target;
use crate::member::{self, ErasedMember, Place};
use crate::source::ErasedStyleSourceType;

/// The expression functions, layer types, source types and extension
/// members that a style's JSON, its layers' property values and its filters
/// parse against, and the registered projections that its `projection.type`
/// can name.
pub struct StyleRegistry {
    functions: Arc<Functions>,
    layer_types: VecMap<&'static str, &'static dyn ErasedStyleLayerType>,
    source_types: VecMap<&'static str, &'static dyn ErasedStyleSourceType>,
    root_members: VecMap<&'static str, &'static dyn ErasedMember>,
    layer_members: VecMap<&'static str, &'static dyn ErasedMember>,
    projections: VecMap<String, Arc<CrsMapping>>,
}

/// A member's JSON value, and its parsed value where the registry has the
/// member.
pub(crate) type MemberValue = (Value, Option<Arc<dyn std::any::Any + Send + Sync>>);

impl StyleRegistry {
    /// The registry of `functions`, `layer_types`, `source_types`, `members`
    /// and `projections`, each projection with the one mapping that every
    /// map drawn in it shares. A type or a projection is known by its name,
    /// and a member by its place and name: of two of one name, the first
    /// counts.
    #[must_use]
    pub fn new(
        functions: Arc<Functions>,
        layer_types: impl IntoIterator<Item = &'static dyn ErasedStyleLayerType>,
        source_types: impl IntoIterator<Item = &'static dyn ErasedStyleSourceType>,
        members: impl IntoIterator<Item = &'static dyn ErasedMember>,
        projections: impl IntoIterator<Item = Arc<Crs>>,
    ) -> Self {
        let mut layers_by_name = VecMap::new();
        for layer_type in layer_types {
            layers_by_name.entry(layer_type.info().name).or_insert(layer_type);
        }
        let mut sources_by_name = VecMap::new();
        for source_type in source_types {
            sources_by_name.entry(source_type.info().name).or_insert(source_type);
        }
        let (mut root_members, mut layer_members) = (VecMap::new(), VecMap::new());
        for member in members {
            let by_name = match member.place() {
                Place::Root => &mut root_members,
                Place::Layer => &mut layer_members,
            };
            by_name.entry(member.name()).or_insert(member);
        }
        let mut projections_by_name = VecMap::new();
        for crs in projections {
            projections_by_name.entry(crs.name.clone()).or_insert_with(|| Arc::new(CrsMapping::new(crs)));
        }
        StyleRegistry {
            functions,
            layer_types: layers_by_name,
            source_types: sources_by_name,
            root_members,
            layer_members,
            projections: projections_by_name,
        }
    }

    /// The built-in expression functions, the style halves of the built-in
    /// layer and source types, and the `identity` projection, for this
    /// crate's tests, which can't reach `mln_builtins::registry`.
    #[cfg(test)]
    pub(crate) fn with_builtins() -> Arc<Self> {
        use crate::layer::style_entry;
        use crate::layers::{
            Background, Circle, ColorRelief, Fill, FillExtrusion, Heatmap, Hillshade, Line, Raster, Symbol,
        };
        use crate::source::style_source_entry;
        use crate::sources::{
            GeoJsonSourceType, ImageSourceType, RasterDemSourceType, RasterSourceType, VectorSourceType,
        };
        let layer_types = [
            style_entry::<Fill>(),
            style_entry::<Line>(),
            style_entry::<Circle>(),
            style_entry::<Symbol>(),
            style_entry::<Raster>(),
            style_entry::<Background>(),
            style_entry::<Hillshade>(),
            style_entry::<ColorRelief>(),
            style_entry::<FillExtrusion>(),
            style_entry::<Heatmap>(),
        ];
        let source_types = [
            style_source_entry::<VectorSourceType>(),
            style_source_entry::<RasterSourceType>(),
            style_source_entry::<RasterDemSourceType>(),
            style_source_entry::<GeoJsonSourceType>(),
            style_source_entry::<ImageSourceType>(),
        ];
        let projections = [Arc::new(Crs::identity())];
        Arc::new(StyleRegistry::new(Functions::with_builtins(), layer_types, source_types, [], projections))
    }

    #[must_use]
    pub fn functions(&self) -> &Arc<Functions> {
        &self.functions
    }

    #[must_use]
    pub fn layer_type(&self, name: &str) -> Option<&'static dyn ErasedStyleLayerType> {
        self.layer_types.get(name).copied()
    }

    /// The layer types, in name order.
    pub fn layer_types(&self) -> impl Iterator<Item = &'static dyn ErasedStyleLayerType> + '_ {
        self.layer_types.values().copied()
    }

    #[must_use]
    pub fn source_type(&self, name: &str) -> Option<&'static dyn ErasedStyleSourceType> {
        self.source_types.get(name).copied()
    }

    /// The source types, in name order.
    pub fn source_types(&self) -> impl Iterator<Item = &'static dyn ErasedStyleSourceType> + '_ {
        self.source_types.values().copied()
    }

    /// The mapping of the registered projection `name`, which a style's
    /// `projection.type` can name.
    #[must_use]
    pub fn projection(&self, name: &str) -> Option<&Arc<CrsMapping>> {
        self.projections.get(name)
    }

    /// The extension member `name` at `place`, if the registry has it.
    #[must_use]
    pub fn member(&self, place: Place, name: &str) -> Option<&'static dyn ErasedMember> {
        let by_name = match place {
            Place::Root => &self.root_members,
            Place::Layer => &self.layer_members,
        };
        by_name.get(name).copied()
    }

    /// The value of the extension member `name` at `place` from `value`, or
    /// `None` for null or undefined, which unsets it. A member the registry
    /// lacks keeps `value` as it is, with a warning.
    ///
    /// # Errors
    ///
    /// When the registry has the member and `value` isn't one of it.
    pub(crate) fn member_value(
        &self,
        place: Place,
        name: &str,
        value: &Convertible<'_>,
    ) -> Result<Option<MemberValue>, Error> {
        if value.is_undefined() {
            return Ok(None);
        }
        let json = member::to_value(value);
        let Some(member) = self.member(place, name) else {
            let at = match place {
                Place::Root => "the style's root",
                Place::Layer => "a layer",
            };
            log::warn!(target: log_target::PARSE_STYLE, "no extension registered the member \"{name}\" of {at}");
            return Ok(Some((json, None)));
        };
        match member.parse(&Convertible::from(&json), &self.functions) {
            Ok(parsed) => Ok(Some((json, Some(parsed)))),
            Err(error) => Err(Error::new(format!("member \"{name}\": {}", error.message))),
        }
    }

    /// A layer of the type `type_` from its style JSON `value`, without its
    /// properties.
    pub(crate) fn create_layer(
        &self,
        type_: &str,
        id: &str,
        value: &Convertible<'_>,
        error: &mut Error,
    ) -> Option<Layer> {
        let Some(layer_type) = self.layer_type(type_) else {
            error.message = format!("Unsupported layer type! Null factory for type: {type_}");
            return None;
        };
        let info = layer_type.info();
        if !info.from_json {
            error.message = format!("layer type '{type_}' can't be created from style JSON");
            return None;
        }
        let source = if info.source == SourceKind::None {
            String::new()
        } else {
            let Some(source) = value.object_member("source").and_then(|source| source.to_string()) else {
                error.message = format!("Error parsing layer {id} of type: {type_}");
                return None;
            };
            source
        };
        Some(Layer::from_style(layer_type.create(id, &source), Arc::clone(&self.functions)))
    }
}

impl std::fmt::Debug for StyleRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyleRegistry")
            .field("layer_types", &self.layer_types.keys().collect::<Vec<_>>())
            .field("source_types", &self.source_types.keys().collect::<Vec<_>>())
            .field("root_members", &self.root_members.keys().collect::<Vec<_>>())
            .field("layer_members", &self.layer_members.keys().collect::<Vec<_>>())
            .field("projections", &self.projections.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// A context in which layers read with [`mln_wire`] resolve their type's
/// name against this registry, and compound expressions their function's
/// against its functions.
impl mln_wire::Context for &StyleRegistry {
    fn provide<'a>(&'a mut self, request: &mut mln_wire::Request<'a>) {
        let registry: &'a StyleRegistry = self;
        request.give(registry).give::<Functions>(registry.functions());
    }
}
