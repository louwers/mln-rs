//! The registrations that a runtime, its renderers and its tile workers
//! share.

use std::sync::Arc;

use mln_style::StyleRegistry;
use mln_style::expression::Functions;
use mln_style::expression::functions::RegisterError;
use mln_style::layer_type::{ErasedStyleLayerType, StyleLayerType};
use mln_style::source_type::ErasedStyleSourceType;
use mln_style::style_key::{StyleKey, StyleKeys};
use mln_vec_map::VecMap;

use crate::erased_layer::{ErasedLayerType, entry};
use crate::erased_source::{self, ErasedSourceType};
use crate::layer::Layer;
use crate::placement::{ErasedPlacement, Placement};
use crate::source::SourceType;

/// A placing layer type's name, and what makes its participant.
type PlacingType = (&'static str, fn() -> Box<dyn ErasedPlacement>);

/// What a [`Registry`] is built from: the expression functions, the layer
/// types, the source types and the style keys.
///
/// An app fills a builder by explicit calls and builds the [`Registry`]
/// once. Tile workers in other wasm instances make the same calls, since
/// they run the app's own module. Each registration is by name and fails
/// for a name taken already ([`RegisterError`]).
#[derive(Default)]
pub struct RegistryBuilder {
    functions: Functions,
    layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
    source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
    style_keys: StyleKeys,
    placing_types: Vec<PlacingType>,
}

impl std::fmt::Debug for RegistryBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistryBuilder")
            .field("functions", &self.functions)
            .field("layer_types", &self.layer_types.keys().collect::<Vec<_>>())
            .field("source_types", &self.source_types.keys().collect::<Vec<_>>())
            .field("style_keys", &self.style_keys)
            .field("placing_types", &self.placing_types.iter().map(|(name, _)| name).collect::<Vec<_>>())
            .finish()
    }
}

impl RegistryBuilder {
    #[must_use]
    pub fn new() -> Self {
        RegistryBuilder::default()
    }

    /// The expression functions, to add to
    /// ([`Functions::add`](mln_style::expression::Functions::add)).
    pub fn functions_mut(&mut self) -> &mut Functions {
        &mut self.functions
    }

    /// Adds the layer type of the layer `L`, both its halves. Fails for a
    /// name that an earlier type took, and for a property named as a style
    /// key. A layer whose
    /// [`Input`](Layer::Input) is of another kind of source than its style
    /// half's doesn't compile.
    pub fn add_layer_type<L: Layer>(&mut self) -> Result<(), RegisterError> {
        self.add_layer_entry::<L>(entry::<L, ()>())
    }

    /// Adds the layer type of the layer `P::Layer`, as
    /// [`add_layer_type`](Self::add_layer_type) does, whose layers place with
    /// the participant `P` ([`placement`](crate::placement)). Each renderer
    /// makes one `P`.
    pub fn add_placing_layer_type<P: Placement>(&mut self) -> Result<(), RegisterError> {
        self.add_layer_entry::<P::Layer>(entry::<P::Layer, P>())?;
        let name = <<P::Layer as Layer>::Style as StyleLayerType>::INFO.name;
        self.placing_types.push((name, crate::placement::erase::<P>));
        Ok(())
    }

    /// Adds the layer type of the layer `L`, as `entry` holds it, as
    /// [`add_layer_type`](Self::add_layer_type) does.
    fn add_layer_entry<L: Layer>(&mut self, entry: &'static dyn ErasedLayerType) -> Result<(), RegisterError> {
        const {
            assert!(
                <L::Style as StyleLayerType>::INFO.source as u8 == crate::layer::input_source::<L>() as u8,
                "a layer's input is of another kind of source than its style half's",
            );
        }
        let name = <L::Style as StyleLayerType>::INFO.name;
        if self.layer_types.contains_key(name) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        self.style_keys.add_layer_type::<L::Style>()?;
        self.layer_types.insert(name, entry);
        Ok(())
    }

    /// Adds the style key `key` ([`StyleKey`]). Adding a key again changes
    /// nothing, so two extensions may both add a key they share. Fails for a
    /// name that another key or a property of a layer type has, and for a
    /// value type that another key has: the style reads a key's value by its
    /// type.
    pub fn add_style_key<T: PartialEq + std::fmt::Debug + Send + Sync + 'static>(
        &mut self,
        key: &'static StyleKey<T>,
    ) -> Result<(), RegisterError> {
        self.style_keys.add(key)
    }

    /// Adds the source type `T`, both its halves. Fails for a name that an
    /// earlier type took, and for a type whose
    /// [`Data`](SourceType::Data) is of another kind than its style half's
    /// [`layer_kind`](mln_style::source_type::SourceTypeInfo::layer_kind).
    pub fn add_source_type<T: SourceType>(&mut self) -> Result<(), RegisterError> {
        let name = T::INFO.name;
        if T::INFO.layer_kind != <T::Data as crate::source::kind::Deliver>::KIND {
            return Err(RegisterError::SourceKind(name.to_owned()));
        }
        if self.source_types.contains_key(name) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        self.source_types.insert(name, erased_source::entry::<T>());
        Ok(())
    }

    #[must_use]
    pub fn build(self) -> Arc<Registry> {
        Registry::new(
            Arc::new(self.functions),
            self.layer_types,
            self.source_types,
            self.style_keys,
            self.placing_types,
        )
    }
}

fn style_half(layer_type: &'static dyn ErasedLayerType) -> &'static dyn ErasedStyleLayerType {
    layer_type.style()
}

fn style_source_half(source_type: &'static dyn ErasedSourceType) -> &'static dyn ErasedStyleSourceType {
    source_type
}

/// The expression functions, layer types, source types and style keys that
/// a style parses against, its renderer draws and its tile workers lay out,
/// built by a [`RegistryBuilder`]. It doesn't change once built.
///
/// A map and the renderer and tile workers that draw it must be given
/// registries filled by the same calls: they identify functions, layer
/// types and source types by name.
pub struct Registry {
    style: Arc<StyleRegistry>,
    layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
    source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
    placing_types: Vec<PlacingType>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registry").field("style", &self.style).finish_non_exhaustive()
    }
}

impl Registry {
    /// The built-in expression functions and the tests' fixture layer and
    /// source types, for this crate's tests, which can't reach the facade's
    /// registry.
    #[cfg(test)]
    pub(crate) fn with_builtins() -> Arc<Registry> {
        use crate::tests::scripted_origin::{Dem, Geometry, Raster, Scripted};
        static BUILTINS: std::sync::LazyLock<Arc<Registry>> = std::sync::LazyLock::new(|| {
            let mut builder = RegistryBuilder::new();
            mln_style::expression::functions::register_builtin_functions(builder.functions_mut()).unwrap();
            builder.add_layer_type::<crate::tests::fixture_layer::Dot>().unwrap();
            builder.add_layer_type::<crate::tests::fixture_layer::Tint>().unwrap();
            builder.add_layer_type::<crate::tests::fixture_layer::Shade>().unwrap();
            builder.add_source_type::<Scripted<Geometry>>().unwrap();
            builder.add_source_type::<Scripted<Raster>>().unwrap();
            builder.add_source_type::<Scripted<Dem>>().unwrap();
            builder.build()
        });
        Arc::clone(&BUILTINS)
    }

    fn new(
        functions: Arc<Functions>,
        layer_types: VecMap<&'static str, &'static dyn ErasedLayerType>,
        source_types: VecMap<&'static str, &'static dyn ErasedSourceType>,
        style_keys: StyleKeys,
        placing_types: Vec<PlacingType>,
    ) -> Arc<Registry> {
        let style_halves = layer_types.values().map(|&layer_type| style_half(layer_type));
        let style_source_halves = source_types.values().map(|&source_type| style_source_half(source_type));
        let style = StyleRegistry::new(functions, style_halves, style_source_halves, style_keys);
        Arc::new(Registry { style: Arc::new(style), layer_types, source_types, placing_types })
    }

    #[must_use]
    pub fn style(&self) -> &Arc<StyleRegistry> {
        &self.style
    }

    #[must_use]
    pub fn functions(&self) -> &Arc<Functions> {
        self.style.functions()
    }

    pub(crate) fn layer_type(&self, name: &str) -> Option<&'static dyn ErasedLayerType> {
        self.layer_types.get(name).copied()
    }

    /// A new participant of each placing layer type, in the order they
    /// registered.
    pub(crate) fn new_placements(&self) -> Vec<Box<dyn ErasedPlacement>> {
        self.placing_types.iter().map(|(_, new)| new()).collect()
    }

    /// The position among [`new_placements`](Self::new_placements) of the
    /// participant of the layer type `name`, if its layers place.
    pub(crate) fn placing_type(&self, name: &str) -> Option<usize> {
        self.placing_types.iter().position(|(placing, _)| *placing == name)
    }

    pub(crate) fn source_type(&self, name: &str) -> Option<&'static dyn ErasedSourceType> {
        self.source_types.get(name).copied()
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
    use mln_style::conversion::Error;
    use mln_style::layer_type::{NoProperties, SourceKind};
    use mln_style::property_table::{PropertyInfo, PropertyTable};
    use mln_style::{LayerTypeInfo, Source};

    use super::*;
    use crate::draw::Draws;
    use crate::layer::{NewCx, PrepareCx, ViewCover};
    use crate::tests::fixture_layer::Dot;
    use crate::tests::scripted_origin::{Geometry, Scripted};

    #[test]
    fn a_source_type_registers_once() {
        let mut builder = RegistryBuilder::new();
        builder.add_source_type::<Scripted<Geometry>>().unwrap();
        assert_eq!(
            builder.add_source_type::<Scripted<Geometry>>(),
            Err(RegisterError::Duplicate("scripted".to_owned()))
        );
    }

    #[test]
    fn a_layer_type_registers_once() {
        let mut builder = RegistryBuilder::new();
        builder.add_layer_type::<Dot>().unwrap();
        assert_eq!(builder.add_layer_type::<Dot>(), Err(RegisterError::Duplicate("dot".to_owned())));
    }

    #[derive(Debug, PartialEq)]
    struct Mark(f64);

    static MARK: StyleKey<Mark> = StyleKey::layer(
        "test:mark",
        |value, _| value.to_double().map(Mark).ok_or_else(|| Error::new("expected a number")),
        |mark| feature::Value::Double(mark.0),
    );

    /// A layer type with a property named as [`MARK`].
    struct Marked;

    impl StyleLayerType for Marked {
        const INFO: LayerTypeInfo = LayerTypeInfo { name: "marked", source: SourceKind::None, from_json: true };
        const PROPERTIES: &'static PropertyTable = &PropertyTable::unsorted(&[PropertyInfo::paint("test:mark")]);
        type LayoutProperties = NoProperties;
        type PaintProperties = NoProperties;
        type Data = ();
    }

    impl Layer for Marked {
        type Style = Marked;
        type Input = ViewCover;
        type Tile = ();

        fn new(_cx: &mut NewCx<'_, Self>) -> Self {
            Marked
        }

        fn prepare(&self, _cx: &PrepareCx<'_, Self>, _out: &mut Draws) {}
    }

    #[test]
    fn a_style_key_registers_once_and_reaches_the_style() {
        let mut builder = RegistryBuilder::new();
        builder.add_layer_type::<Dot>().unwrap();
        builder.add_style_key(&MARK).unwrap();
        builder.add_style_key(&MARK).unwrap();
        let registry = builder.build();
        let json = r#"{"version":8,"layers":[{"id":"d","type":"dot","source":"s","test:mark":2}]}"#;
        let style = mln_style::Style::from_json(registry.style(), json, None).unwrap();
        assert_eq!(Some(&Mark(2.0)), style.get_layer("d").unwrap().style().base().extra::<Mark>());
    }

    #[test]
    fn a_style_key_and_a_layer_property_cant_share_a_name() {
        let duplicate = Err(RegisterError::Duplicate("test:mark".to_owned()));
        let mut builder = RegistryBuilder::new();
        builder.add_layer_type::<Marked>().unwrap();
        assert_eq!(duplicate, builder.add_style_key(&MARK));

        let mut builder = RegistryBuilder::new();
        builder.add_style_key(&MARK).unwrap();
        assert_eq!(duplicate, builder.add_layer_type::<Marked>());
        assert!(builder.build().layer_type("marked").is_none());
    }
}
