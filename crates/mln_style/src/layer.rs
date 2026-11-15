// A layer cannot point back at the style that owns it, so `Style::Impl` hands
// out layers through `LayerRefMut` guards, which run
// `Style::Impl::on_layer_changed` when the guard is dropped if the layer's
// `base_impl` changed. A change copies the impl when it is shared, as it is
// while the guard holds the impl from before, and leaves it alone when the
// value doesn't change. Several changes through one guard therefore notify
// once.

use std::sync::Arc;

use feature::{Value, ValueObject};
use mln_conversion::{Convertible, Error};
use mln_style_types::VisibilityType;
use mln_util::logging::{Event, Log};

use crate::conversion::{Converter, convert, convert_filter};
use crate::expression::{
    Dependency, EvaluationContext, Expr, Functions, GlobalStateMap, ParsingContext, Type, from_expression_value,
};
use crate::filter::Filter;
pub(crate) use crate::layer_impl::FontStack;
pub use crate::layer_impl::{Impl, ImplBase, downcast_arc, font_stacks};
use crate::layer_type::LayerStyle;
pub use crate::layer_type::{LayerTypeInfo, SourceKind};
use crate::layers::Custom;
use crate::layers::custom_layer::CustomLayerHost;
use crate::property_table::SetPropertyError;
use crate::registry::StyleRegistry;
use crate::style_property::StyleProperty;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyScope {
    Paint,
    Layout,
}

/// A layer from the MapLibre Style Spec, of any layer type: its style data,
/// and the expression functions its property values parse against.
pub struct Layer {
    base_impl: Arc<dyn Impl>,
    functions: Arc<Functions>,
    /// A custom layer's host, which the style data only refers to.
    host: Option<Arc<dyn CustomLayerHost>>,
}

impl Layer {
    /// A layer of the type named `type_` in `registry`, named `id`, of the
    /// source `source`, with every property undefined.
    ///
    /// # Errors
    ///
    /// When `registry` has no layer type named `type_`, or one that only
    /// another constructor makes, such as [`Layer::custom`].
    pub fn new(registry: &StyleRegistry, type_: &str, id: &str, source: &str) -> Result<Layer, mln_util::Error> {
        let Some(layer_type) = registry.layer_type(type_) else {
            return Err(mln_util::Error::invalid_argument(format!("unknown layer type '{type_}'")));
        };
        if !layer_type.info().from_json {
            return Err(mln_util::Error::invalid_argument(format!("layer type '{type_}' can't be created by name")));
        }
        Ok(Layer::from_style(layer_type.create(id, source), Arc::clone(registry.functions())))
    }

    /// A layer named `id` that `host` draws, whose filter and visibility
    /// parse against `registry`'s functions.
    #[must_use]
    pub fn custom(registry: &StyleRegistry, id: &str, host: Arc<dyn CustomLayerHost>) -> Layer {
        Layer {
            base_impl: Arc::new(LayerStyle::<Custom>::with_host(id.to_owned(), &host)),
            functions: Arc::clone(registry.functions()),
            host: Some(host),
        }
    }

    /// The layer of `style`, whose property values parse against
    /// `functions`. A custom layer needs its host: [`Layer::custom`] makes
    /// one.
    #[must_use]
    pub fn from_style(style: Arc<dyn Impl>, functions: Arc<Functions>) -> Layer {
        Layer { base_impl: style, functions, host: None }
    }

    /// The style data.
    #[must_use]
    pub fn base_impl(&self) -> Arc<dyn Impl> {
        Arc::clone(&self.base_impl)
    }

    /// The style data, borrowed: reading it this way skips the reference
    /// count.
    #[must_use]
    pub fn base_impl_ref(&self) -> &dyn Impl {
        &*self.base_impl
    }

    /// Changes the base members, copying the style data if it is shared.
    fn mutate_base_impl(&mut self, f: &mut dyn FnMut(&mut ImplBase)) {
        self.base_impl.layer_type().mutate_base(&mut self.base_impl, f);
    }

    #[must_use]
    pub fn get_property(&self, name: &str) -> StyleProperty {
        self.base_impl.get_property(name)
    }

    /// The expression functions that the layer's property values, filter and
    /// visibility parse against.
    #[must_use]
    pub fn functions(&self) -> &Arc<Functions> {
        &self.functions
    }

    /// Serializes the layer as a style JSON value.
    #[must_use]
    pub fn serialize(&self) -> Value {
        let mut result = ValueObject::new();
        result.insert("id".to_owned(), Value::String(self.get_id()));
        result.insert("type".to_owned(), Value::String(self.get_type_info().name.to_owned()));

        let source = self.get_source_id();
        if !source.is_empty() {
            result.insert("source".to_owned(), Value::String(source));
        }

        let source_layer = self.get_source_layer();
        if !source_layer.is_empty() {
            result.insert("source-layer".to_owned(), Value::String(source_layer));
        }

        if self.get_filter().is_set() {
            result.insert("filter".to_owned(), self.get_filter().serialize());
        }

        if self.get_min_zoom() != f32::NEG_INFINITY {
            result.insert("minzoom".to_owned(), Value::Double(f64::from(self.get_min_zoom())));
        }

        if self.get_max_zoom() != f32::INFINITY {
            result.insert("maxzoom".to_owned(), Value::Double(f64::from(self.get_max_zoom())));
        }

        if let Some(expression) = &self.base_impl.visibility_expression {
            let mut layout = ValueObject::new();
            layout.insert("visibility".to_owned(), expression.serialize());
            result.insert("layout".to_owned(), Value::Object(layout));
        } else if self.get_visibility() == VisibilityType::None {
            let mut layout = ValueObject::new();
            layout.insert("visibility".to_owned(), Value::String("none".to_owned()));
            result.insert("layout".to_owned(), Value::Object(layout));
        }

        self.base_impl.serialize_properties(&mut result);
        Value::Object(result)
    }

    /// A layer named `id` with this layer's members and properties but its
    /// paint properties. A custom layer's copy draws through the same host.
    #[must_use]
    pub fn clone_ref(&self, id: &str) -> Layer {
        Layer {
            base_impl: self.base_impl.copy_without_paint(id),
            functions: Arc::clone(&self.functions),
            host: self.host.clone(),
        }
    }

    /// Sets the property `name` (a property of the layer's type, its
    /// `<name>-transition`, or a member every layer has) from a style JSON
    /// value. Returns why it fails.
    pub fn set_property(
        &mut self,
        name: &str,
        value: &Convertible<'_>,
        _scope: Option<PropertyScope>,
    ) -> Option<Error> {
        let layer_type = self.base_impl.layer_type();
        let mut error = match layer_type.set_property(&mut self.base_impl, name, value, &self.functions) {
            Ok(()) => return None,
            Err(SetPropertyError::Invalid(error)) => error,
            Err(SetPropertyError::Unknown) => {
                Error::new(format!("layer '{}' doesn't support property '{}'", self.get_id(), name))
            }
        };
        if name == "visibility" {
            return self.set_visibility_value(value);
        }
        if name == "minzoom" {
            if let Some(zoom) = convert::<f32>(value, &mut error) {
                self.set_min_zoom(zoom);
                return None;
            }
        } else if name == "maxzoom" {
            if let Some(zoom) = convert::<f32>(value, &mut error) {
                self.set_max_zoom(zoom);
                return None;
            }
        } else if name == "filter" {
            if let Some(filter) = convert_filter(value, &self.functions, &mut error) {
                self.set_filter(&filter);
                return None;
            }
        } else if name == "source-layer" {
            if let Some(source_layer) = convert::<String>(value, &mut error) {
                if self.get_type_info().source == SourceKind::None {
                    // Quirk kept: no space between "to" and "the".
                    Log::warning(
                        Event::General,
                        format!("'source-layer' property cannot be set tothe layer {}", self.base_impl.id),
                    );
                    return None;
                }
                self.set_source_layer(&source_layer);
                return None;
            }
        } else if name == "source"
            && let Some(source_id) = convert::<String>(value, &mut error)
        {
            if self.get_type_info().source == SourceKind::None {
                Log::warning(
                    Event::General,
                    format!("'source' property cannot be set tothe layer {}", self.base_impl.id),
                );
                return None;
            }
            self.set_source_id(&source_id);
            return None;
        }
        Some(error)
    }

    fn set_visibility_value(&mut self, value: &Convertible<'_>) -> Option<Error> {
        if value.is_undefined() {
            self.set_visibility(VisibilityType::Visible);
            return None;
        }

        if value.is_array() {
            // Per the style specification, visibility expressions may only use "global-state".
            let mut ctx = ParsingContext::with_expected(&self.functions, Type::String);
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
            let expression: Arc<Expr> = Arc::from(parsed);
            self.mutate_base_impl(&mut |impl_| impl_.visibility_expression = Some(Arc::clone(&expression)));
            return None;
        }

        let mut error = Error::default();
        let visibility = <VisibilityType as Converter>::convert(value, &mut error);
        match visibility {
            Some(visibility) => {
                self.set_visibility(visibility);
                None
            }
            None => Some(error),
        }
    }

    #[must_use]
    pub fn get_id(&self) -> String {
        self.base_impl.id.clone()
    }

    #[must_use]
    pub fn get_source_id(&self) -> String {
        self.base_impl.source.clone()
    }

    #[must_use]
    pub fn get_source_layer(&self) -> String {
        self.base_impl.source_layer.clone()
    }

    pub fn set_source_layer(&mut self, source_layer: &str) {
        if self.base_impl.source_layer == source_layer {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.source_layer = source_layer.to_owned());
    }

    pub fn set_source_id(&mut self, source_id: &str) {
        if self.base_impl.source == source_id {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.source = source_id.to_owned());
    }

    #[must_use]
    pub fn get_filter(&self) -> Filter {
        self.base_impl.filter.clone()
    }

    pub fn set_filter(&mut self, filter: &Filter) {
        if self.base_impl.filter == *filter {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.filter = filter.clone());
    }

    #[must_use]
    pub fn get_visibility(&self) -> VisibilityType {
        self.base_impl.visibility
    }

    pub fn set_visibility(&mut self, value: VisibilityType) {
        if value == self.get_visibility() && self.base_impl.visibility_expression.is_none() {
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
    pub fn reevaluate_visibility(&mut self, global_state: &GlobalStateMap) {
        let Some(expression) = &self.base_impl.visibility_expression else {
            return;
        };
        let value = evaluate_visibility_expression(expression.as_ref(), global_state);
        if value == self.base_impl.visibility {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.visibility = value);
    }

    /// The serialized visibility expression, or a null value when the
    /// visibility is a constant.
    #[must_use]
    pub fn get_visibility_expression(&self) -> Value {
        match &self.base_impl.visibility_expression {
            Some(expression) => expression.serialize(),
            None => Value::Null,
        }
    }

    #[must_use]
    pub fn get_min_zoom(&self) -> f32 {
        self.base_impl.min_zoom
    }

    #[must_use]
    pub fn get_max_zoom(&self) -> f32 {
        self.base_impl.max_zoom
    }

    pub fn set_min_zoom(&mut self, min_zoom: f32) {
        if self.get_min_zoom() == min_zoom {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.min_zoom = min_zoom);
    }

    pub fn set_max_zoom(&mut self, max_zoom: f32) {
        if self.get_max_zoom() == max_zoom {
            return;
        }
        self.mutate_base_impl(&mut |impl_| impl_.max_zoom = max_zoom);
    }

    #[must_use]
    pub fn get_type_info(&self) -> LayerTypeInfo {
        self.base_impl.get_type_info()
    }
}

fn evaluate_visibility_expression(expression: &Expr, global_state: &GlobalStateMap) -> VisibilityType {
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

// Prints what tells layers apart rather than every property.
impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer")
            .field("id", &self.base_impl.id)
            .field("type", &self.get_type_info().name)
            .finish_non_exhaustive()
    }
}
