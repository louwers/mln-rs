//! Layers of any type: a style's [`Layer`]s and their list ([`LayerList`]),
//! their conversion from style JSON ([`convert_layer`]), and the contract a
//! layer type implements ([`StyleLayerType`]).

// A layer cannot point back at the style that owns it, so a style edits a
// copy of its layer (`Style::update_layer`) and keeps the copy if the
// layer's style data changed. A change copies the style data when it is
// shared, as it is while the style still holds the original, and leaves it
// alone when the value doesn't change.

use std::any::Any;
use std::sync::Arc;

use feature::{Value, ValueObject};
use mln_vec_map::VecSet;

use crate::conversion::{Converter, Convertible, Error, convert};
use crate::expression::{
    Dependency, EvaluationContext, Expr, Functions, GlobalStateMap, ParsingContext, Type, from_expression_value,
};
use crate::filter::{Filter, convert_filter};
use crate::log_target;
use crate::property::table::{SetPropertyError, StyleProperty};
use crate::registry::StyleRegistry;
use crate::types::VisibilityType;

mod convert;
mod list;
mod type_;

pub use convert::convert_layer;
pub(crate) use convert::{InvalidProperty, convert_layer_with, set_paint_properties};
pub use list::LayerList;
pub use type_::{
    ErasedLayerStyle, ErasedStyleLayerType, LayerBase, LayerStyle, LayerTypeEntry, LayerTypeInfo, LayoutSet,
    NoProperties, PaintSet, SourceKind, StyleLayerType, style_entry,
};

pub(crate) type FontStack = Vec<String>;

/// The font stacks `layers` use, found by statically evaluating their
/// `text-font` values.
pub fn font_stacks<L: AsRef<dyn ErasedLayerStyle>>(layers: impl IntoIterator<Item = L>) -> VecSet<FontStack> {
    let mut result = VecSet::new();
    for layer in layers {
        layer.as_ref().populate_font_stack(&mut result);
    }
    result
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyScope {
    Paint,
    Layout,
}

/// A layer from the MapLibre Style Spec, of any layer type: its style data,
/// and the expression functions its property values parse against. A clone
/// shares them until either changes.
#[derive(Clone)]
pub struct Layer {
    style: Arc<dyn ErasedLayerStyle>,
    functions: Arc<Functions>,
    /// What the layer keeps alive for its style data, which only refers to
    /// it (see [`Layer::from_style_holding`]).
    held: Option<Arc<dyn Any + Send + Sync>>,
}

impl Layer {
    /// A layer of the type named `type_` in `registry`, named `id`, of the
    /// source `source`, with every property undefined.
    ///
    /// # Errors
    ///
    /// When `registry` has no layer type named `type_`, or one whose layers
    /// can't be made by name, such as a type whose style data needs an
    /// object from the caller.
    pub fn new(registry: &StyleRegistry, type_: &str, id: &str, source: &str) -> Result<Layer, crate::error::Error> {
        let Some(layer_type) = registry.layer_type(type_) else {
            return Err(crate::error::Error::invalid_argument(format!("unknown layer type '{type_}'")));
        };
        if !layer_type.info().from_json {
            return Err(crate::error::Error::invalid_argument(format!(
                "layer type '{type_}' can't be created by name"
            )));
        }
        Ok(Layer::from_style(layer_type.create(id, source), Arc::clone(registry.functions())))
    }

    /// The layer of `style`, whose property values parse against
    /// `functions`.
    #[must_use]
    pub fn from_style(style: Arc<dyn ErasedLayerStyle>, functions: Arc<Functions>) -> Layer {
        Layer { style, functions, held: None }
    }

    /// The layer of `style`, as [`Layer::from_style`] makes, that also keeps
    /// `held` alive while it or a copy of it lives: for style data that
    /// refers to an object only weakly, so that a renderer holding the data
    /// doesn't keep the object.
    #[must_use]
    pub fn from_style_holding(
        style: Arc<dyn ErasedLayerStyle>,
        functions: Arc<Functions>,
        held: Arc<dyn Any + Send + Sync>,
    ) -> Layer {
        Layer { style, functions, held: Some(held) }
    }

    #[must_use]
    pub fn style(&self) -> &Arc<dyn ErasedLayerStyle> {
        &self.style
    }

    /// Changes the base members, copying the style data if it is shared.
    fn mutate_base(&mut self, f: &mut dyn FnMut(&mut LayerBase)) {
        self.style.layer_type().mutate_base(&mut self.style, f);
    }

    #[must_use]
    pub fn get_property(&self, name: &str) -> StyleProperty {
        self.style.get_property(name)
    }

    /// The expression functions that the layer's property values, filter and
    /// visibility parse against.
    #[must_use]
    pub fn functions(&self) -> &Arc<Functions> {
        &self.functions
    }

    #[must_use]
    pub fn serialize(&self) -> Value {
        let mut result = ValueObject::new();
        result.insert("id".to_owned(), Value::String(self.get_id().to_owned()));
        result.insert("type".to_owned(), Value::String(self.info().name.to_owned()));

        let source = self.get_source_id();
        if !source.is_empty() {
            result.insert("source".to_owned(), Value::String(source.to_owned()));
        }

        let source_layer = self.get_source_layer();
        if !source_layer.is_empty() {
            result.insert("source-layer".to_owned(), Value::String(source_layer.to_owned()));
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

        if let Some(expression) = &self.style.base().visibility_expression {
            let mut layout = ValueObject::new();
            layout.insert("visibility".to_owned(), expression.serialize());
            result.insert("layout".to_owned(), Value::Object(layout));
        } else if self.get_visibility() == VisibilityType::None {
            let mut layout = ValueObject::new();
            layout.insert("visibility".to_owned(), Value::String("none".to_owned()));
            result.insert("layout".to_owned(), Value::Object(layout));
        }

        self.style.serialize_properties(&mut result);
        Value::Object(result)
    }

    /// A layer named `id` with this layer's members and properties but its
    /// paint properties, which keeps alive what this layer does.
    #[must_use]
    pub fn clone_ref(&self, id: &str) -> Layer {
        Layer {
            style: self.style.copy_without_paint(id),
            functions: Arc::clone(&self.functions),
            held: self.held.clone(),
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
        let layer_type = self.style.layer_type();
        let mut error = match layer_type.set_property(&mut self.style, name, value, &self.functions) {
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
                if self.info().source == SourceKind::None {
                    // Quirk kept: no space between "to" and "the".
                    log::warn!(
                        target: log_target::GENERAL,
                        "'source-layer' property cannot be set tothe layer {}",
                        self.style.base().id
                    );
                    return None;
                }
                self.set_source_layer(&source_layer);
                return None;
            }
        } else if name == "source"
            && let Some(source_id) = convert::<String>(value, &mut error)
        {
            if self.info().source == SourceKind::None {
                log::warn!(
                    target: log_target::GENERAL,
                    "'source' property cannot be set tothe layer {}",
                    self.style.base().id
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
            // is added or changed.
            let expression: Arc<Expr> = Arc::from(parsed);
            self.mutate_base(&mut |impl_| impl_.visibility_expression = Some(Arc::clone(&expression)));
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
    pub fn get_id(&self) -> &str {
        &self.style.base().id
    }

    #[must_use]
    pub fn get_source_id(&self) -> &str {
        &self.style.base().source
    }

    #[must_use]
    pub fn get_source_layer(&self) -> &str {
        &self.style.base().source_layer
    }

    pub fn set_source_layer(&mut self, source_layer: &str) {
        if self.style.base().source_layer == source_layer {
            return;
        }
        self.mutate_base(&mut |impl_| impl_.source_layer = source_layer.to_owned());
    }

    pub fn set_source_id(&mut self, source_id: &str) {
        if self.style.base().source == source_id {
            return;
        }
        self.mutate_base(&mut |impl_| impl_.source = source_id.to_owned());
    }

    #[must_use]
    pub fn get_filter(&self) -> Filter {
        self.style.base().filter.clone()
    }

    pub fn set_filter(&mut self, filter: &Filter) {
        if self.style.base().filter == *filter {
            return;
        }
        self.mutate_base(&mut |impl_| impl_.filter = filter.clone());
    }

    #[must_use]
    pub fn get_visibility(&self) -> VisibilityType {
        self.style.base().visibility
    }

    pub fn set_visibility(&mut self, value: VisibilityType) {
        if value == self.get_visibility() && self.style.base().visibility_expression.is_none() {
            return;
        }
        self.mutate_base(&mut |impl_| {
            impl_.visibility = value;
            // Setting a constant visibility replaces any visibility expression.
            impl_.visibility_expression = None;
        });
    }

    /// Re-evaluates the layer's visibility expression (a "visibility" layout
    /// value using "global-state"), if any, against the given global state.
    pub fn reevaluate_visibility(&mut self, global_state: &GlobalStateMap) {
        let Some(expression) = &self.style.base().visibility_expression else {
            return;
        };
        let value = evaluate_visibility_expression(expression.as_ref(), global_state);
        if value == self.style.base().visibility {
            return;
        }
        self.mutate_base(&mut |impl_| impl_.visibility = value);
    }

    /// The serialized visibility expression, or a null value when the
    /// visibility is a constant.
    #[must_use]
    pub fn get_visibility_expression(&self) -> Value {
        match &self.style.base().visibility_expression {
            Some(expression) => expression.serialize(),
            None => Value::Null,
        }
    }

    #[must_use]
    pub fn get_min_zoom(&self) -> f32 {
        self.style.base().min_zoom
    }

    #[must_use]
    pub fn get_max_zoom(&self) -> f32 {
        self.style.base().max_zoom
    }

    pub fn set_min_zoom(&mut self, min_zoom: f32) {
        if self.get_min_zoom() == min_zoom {
            return;
        }
        self.mutate_base(&mut |impl_| impl_.min_zoom = min_zoom);
    }

    pub fn set_max_zoom(&mut self, max_zoom: f32) {
        if self.get_max_zoom() == max_zoom {
            return;
        }
        self.mutate_base(&mut |impl_| impl_.max_zoom = max_zoom);
    }

    #[must_use]
    pub fn info(&self) -> LayerTypeInfo {
        self.style.info()
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
    log::warn!(
        target: log_target::PARSE_STYLE,
        r#"visibility expression did not evaluate to "visible" or "none"; defaulting to "visible""#
    );
    VisibilityType::Visible
}

// Prints what tells layers apart rather than every property.
impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer")
            .field("id", &self.style.base().id)
            .field("type", &self.info().name)
            .finish_non_exhaustive()
    }
}
