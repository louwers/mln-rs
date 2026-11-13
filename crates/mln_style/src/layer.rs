// C++ `Layer` is an abstract class: the virtual members are the required
// methods of the `Layer` trait, the non-virtual ones are provided methods.
//
// Deviation (structural, no behaviour change): C++ layers hold a
// `LayerObserver*` (the owning `Style::Impl`) and every setter calls
// `observer->onLayerChanged(*this)`. A Rust layer cannot point back at the
// collection that owns it, so `Style::Impl` hands out layers through
// `LayerRefMut` guards, which run `Style::Impl::onLayerChanged` when the
// guard is dropped if the layer's `base_impl` changed (every C++ setter that
// notifies also replaces `baseImpl`). Several setter calls through one guard
// therefore notify once.

use std::any::Any;
use std::sync::Arc;

use feature::{Value, ValueObject};
use mln_conversion::{Convertible, Error};
use mln_style_types::VisibilityType;
use mln_util::logging::{Event, Log};

use crate::conversion::{Converter, convert};
use crate::expression::{
    Dependency, EvaluationContext, Expression, GlobalStateMap, ParsingContext, Type, from_expression_value,
};
use crate::filter::Filter;
pub use crate::layer_impl::{FontStack, Impl, ImplBase, downcast_arc};
use crate::style_property::StyleProperty;

/// The enums nested in C++ `LayerTypeInfo`.
pub mod layer_type_info {
    /// Whether the layer type requires a source.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Source {
        Required,
        NotRequired,
    }

    /// Whether the layer type requires the 3D rendering pass.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Pass3D {
        Required,
        NotRequired,
    }

    /// Whether the layer type requires layouting.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Layout {
        Required,
        NotRequired,
    }

    /// Whether the layer type requires rendering on fading tiles.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum FadingTiles {
        Required,
        NotRequired,
    }

    /// Whether the layer type requires cross-tile indexing and placement.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum CrossTileIndex {
        Required,
        NotRequired,
    }

    /// The supported tile type; the values correspond to `Tile::Kind`.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    #[repr(u8)]
    pub enum TileKind {
        Geometry,
        Raster,
        RasterDEM,
        NotRequired,
    }
}

/// Holds static data for a certain layer type. Compared by address, as in
/// C++ (`impl->getTypeInfo() == BackgroundLayer::Impl::staticTypeInfo()`):
/// use `std::ptr::eq`.
#[derive(Debug)]
pub struct LayerTypeInfo {
    /// The layer type as defined in the style specification.
    pub type_: &'static str,
    pub source: layer_type_info::Source,
    pub pass3d: layer_type_info::Pass3D,
    pub layout: layer_type_info::Layout,
    pub fading_tiles: layer_type_info::FadingTiles,
    pub cross_tile_index: layer_type_info::CrossTileIndex,
    pub tile_kind: layer_type_info::TileKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyScope {
    Paint,
    Layout,
}

/// The runtime representation of a layer from the MapLibre Style Spec.
pub trait Layer: Any + Send + Sync {
    // --- virtual / protected members ---

    /// The `baseImpl` member.
    fn base_impl(&self) -> Arc<dyn Impl>;

    /// Calls `f` on a copy of the base members and stores the result as the
    /// new impl.
    fn mutate_base_impl(&mut self, f: &mut dyn FnMut(&mut ImplBase));

    fn get_property(&self, name: &str) -> StyleProperty;

    /// Serializes the layer as a style JSON value; implementations start from
    /// [`serialize_base`].
    fn serialize(&self) -> Value;

    /// Create a layer, copying all properties except id and paint properties
    /// from this layer.
    fn clone_ref(&self, id: &str) -> Box<dyn Layer>;

    fn set_property_internal(&mut self, name: &str, value: &Convertible<'_>) -> Option<Error>;

    fn set_property(&mut self, name: &str, value: &Convertible<'_>, scope: Option<PropertyScope>) -> Option<Error> {
        set_property_base(self, name, value, scope)
    }

    // --- non-virtual members ---

    fn get_id(&self) -> String {
        self.base_impl().id.clone()
    }

    fn get_source_id(&self) -> String {
        self.base_impl().source.clone()
    }

    fn get_source_layer(&self) -> String {
        self.base_impl().source_layer.clone()
    }

    fn set_source_layer(&mut self, source_layer: &str) {
        if self.get_source_layer() == source_layer {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.source_layer = source_layer.to_owned());
    }

    fn set_source_id(&mut self, source_id: &str) {
        if self.get_source_id() == source_id {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.source = source_id.to_owned());
    }

    fn get_filter(&self) -> Filter {
        self.base_impl().filter.clone()
    }

    fn set_filter(&mut self, filter: &Filter) {
        if self.get_filter() == *filter {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.filter = filter.clone());
    }

    fn get_visibility(&self) -> VisibilityType {
        self.base_impl().visibility
    }

    fn set_visibility(&mut self, value: VisibilityType) {
        if value == self.get_visibility() && self.base_impl().visibility_expression.is_none() {
            return;
        }
        self.mutate_base_impl(&mut |impl_| {
            impl_.visibility = value;
            // Setting a constant visibility replaces any visibility expression.
            impl_.visibility_expression = None;
        });
    }

    /// Re-evaluates the layer's visibility expression (a "visibility" layout
    /// value using "global-state"), if any, against the given global state.
    fn reevaluate_visibility(&mut self, global_state: &GlobalStateMap) {
        let Some(expression) = self.base_impl().visibility_expression.clone() else {
            return;
        };
        let value = evaluate_visibility_expression(expression.as_ref(), global_state);
        if value == self.base_impl().visibility {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.visibility = value);
    }

    /// The serialized visibility expression, or a null value when the
    /// visibility is a constant.
    fn get_visibility_expression(&self) -> Value {
        match &self.base_impl().visibility_expression {
            Some(expression) => expression.serialize(),
            None => Value::Null,
        }
    }

    fn get_min_zoom(&self) -> f32 {
        self.base_impl().min_zoom
    }

    fn get_max_zoom(&self) -> f32 {
        self.base_impl().max_zoom
    }

    fn set_min_zoom(&mut self, min_zoom: f32) {
        if self.get_min_zoom() == min_zoom {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.min_zoom = min_zoom);
    }

    fn set_max_zoom(&mut self, max_zoom: f32) {
        if self.get_max_zoom() == max_zoom {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.max_zoom = max_zoom);
    }

    fn get_type_info(&self) -> &'static LayerTypeInfo {
        self.base_impl().get_type_info()
    }

    /// Collect dependencies
    fn get_dependencies(&self) -> Dependency {
        self.base_impl().get_dependencies()
    }
}

impl dyn Layer {
    /// `layer->as<FooLayer>()`-style checked downcast.
    pub fn downcast_ref<T: Layer>(&self) -> Option<&T> {
        (self as &dyn Any).downcast_ref()
    }

    pub fn downcast_mut<T: Layer>(&mut self) -> Option<&mut T> {
        (self as &mut dyn Any).downcast_mut()
    }
}

fn evaluate_visibility_expression(expression: &dyn Expression, global_state: &GlobalStateMap) -> VisibilityType {
    let result = expression.evaluate(&EvaluationContext::default().with_global_state(Some(global_state)));
    if let Ok(result) = result
        && let Some(string) = from_expression_value::<String>(&result)
        && let Ok(visibility) = string.parse::<VisibilityType>()
    {
        return visibility;
    }
    Log::warning(
        Event::ParseStyle,
        r#"visibility expression did not evaluate to "visible" or "none"; defaulting to "visible""#,
    );
    VisibilityType::Visible
}

/// Serializes the members every layer type has, for `Layer::serialize` to
/// extend.
pub fn serialize_base(layer: &(impl Layer + ?Sized)) -> Value {
    let mut result = ValueObject::new();
    result.insert("id".to_owned(), Value::String(layer.get_id()));
    result.insert("type".to_owned(), Value::String(layer.get_type_info().type_.to_owned()));

    let source = layer.get_source_id();
    if !source.is_empty() {
        result.insert("source".to_owned(), Value::String(source));
    }

    let source_layer = layer.get_source_layer();
    if !source_layer.is_empty() {
        result.insert("source-layer".to_owned(), Value::String(source_layer));
    }

    if layer.get_filter().is_set() {
        result.insert("filter".to_owned(), layer.get_filter().serialize());
    }

    if layer.get_min_zoom() != f32::NEG_INFINITY {
        // mapbox::base::Value from float: double.
        result.insert("minzoom".to_owned(), Value::Double(f64::from(layer.get_min_zoom())));
    }

    if layer.get_max_zoom() != f32::INFINITY {
        result.insert("maxzoom".to_owned(), Value::Double(f64::from(layer.get_max_zoom())));
    }

    let base_impl = layer.base_impl();
    if let Some(expression) = &base_impl.visibility_expression {
        let mut layout = ValueObject::new();
        layout.insert("visibility".to_owned(), expression.serialize());
        result.insert("layout".to_owned(), Value::Object(layout));
    } else if layer.get_visibility() == VisibilityType::None {
        let mut layout = ValueObject::new();
        layout.insert("visibility".to_owned(), Value::String("none".to_owned()));
        result.insert("layout".to_owned(), Value::Object(layout));
    }

    Value::Object(result)
}

pub fn serialize_property(out: &mut Value, property: &StyleProperty, property_name: &str, is_paint: bool) {
    let Value::Object(object) = out else {
        debug_assert!(false, "serialize_property: not an object");
        return;
    };
    let property_type = if is_paint { "paint" } else { "layout" };
    if let Some(Value::Object(existing)) = object.get_mut(property_type) {
        // emplace: does not overwrite an existing key.
        existing.entry(property_name.to_owned()).or_insert_with(|| property.get_value().clone());
    } else {
        let mut inner = ValueObject::new();
        inner.insert(property_name.to_owned(), property.get_value().clone());
        object.insert(property_type.to_owned(), Value::Object(inner));
    }
}

pub fn set_property_base(
    layer: &mut (impl Layer + ?Sized),
    name: &str,
    value: &Convertible<'_>,
    _scope: Option<PropertyScope>,
) -> Option<Error> {
    let Some(mut error) = layer.set_property_internal(name, value) else {
        return None; // Successfully set by the derived class implementation.
    };
    if name == "visibility" {
        return set_visibility_value(layer, value);
    }
    if name == "minzoom" {
        if let Some(zoom) = convert::<f32>(value, &mut error) {
            layer.set_min_zoom(zoom);
            return None;
        }
    } else if name == "maxzoom" {
        if let Some(zoom) = convert::<f32>(value, &mut error) {
            layer.set_max_zoom(zoom);
            return None;
        }
    } else if name == "filter" {
        if let Some(filter) = convert::<Filter>(value, &mut error) {
            layer.set_filter(&filter);
            return None;
        }
    } else if name == "source-layer" {
        if let Some(source_layer) = convert::<String>(value, &mut error) {
            if layer.get_type_info().source != layer_type_info::Source::Required {
                // Quirk kept: no space between "to" and "the".
                Log::warning(
                    Event::General,
                    format!("'source-layer' property cannot be set tothe layer {}", layer.base_impl().id),
                );
                return None;
            }
            layer.set_source_layer(&source_layer);
            return None;
        }
    } else if name == "source"
        && let Some(source_id) = convert::<String>(value, &mut error)
    {
        if layer.get_type_info().source != layer_type_info::Source::Required {
            Log::warning(
                Event::General,
                format!("'source' property cannot be set tothe layer {}", layer.base_impl().id),
            );
            return None;
        }
        layer.set_source_id(&source_id);
        return None;
    }
    Some(error)
}

fn set_visibility_value(layer: &mut (impl Layer + ?Sized), value: &Convertible<'_>) -> Option<Error> {
    if value.is_undefined() {
        layer.set_visibility(VisibilityType::Visible);
        return None;
    }

    if value.is_array() {
        // Per the style specification, visibility expressions may only use "global-state".
        let mut ctx = ParsingContext::with_expected(Type::String);
        let Some(parsed) = ctx.parse_expression(value, None) else {
            return Some(Error::new(ctx.get_combined_errors()));
        };
        let allowed = Dependency::GLOBAL_STATE | Dependency::BIND | Dependency::VAR;
        if (parsed.dependencies() | allowed) != allowed {
            return Some(Error::new(r#"visibility expressions may only use the "global-state" expression"#));
        }

        // The layer may not belong to a style yet, so its global state is not
        // available here. The style reevaluates the expression when the layer
        // is added or when this change reaches its observer.
        let expression: Arc<dyn Expression> = Arc::from(parsed);
        layer.mutate_base_impl(&mut |impl_| impl_.visibility_expression = Some(expression.clone()));
        return None;
    }

    let mut error = Error::default();
    let visibility = <VisibilityType as Converter>::convert(value, &mut error);
    match visibility {
        Some(visibility) => {
            layer.set_visibility(visibility);
            None
        }
        None => Some(error),
    }
}

// Debug is not a supertrait of this trait, nor of the other layer and source
// traits that have a `dyn` Debug impl like this one: through the vtable it
// would keep every implementation's derived Debug (and those of everything it
// holds) in the binary. Each prints what tells its values apart instead.
impl std::fmt::Debug for dyn Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let base = self.base_impl();
        f.debug_struct("Layer").field("id", &base.id).field("type", &base.get_type_info().type_).finish_non_exhaustive()
    }
}
