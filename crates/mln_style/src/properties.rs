// Ports src/mln/style/properties.hpp: `Transitioning<Value>`,
// `Transitionable<Value>` and the `Properties<Ps...>` template.
//
// C++ `Properties<Ps...>` builds four tuples indexed by property tag types
// (`Transitionable`, `Unevaluated`, `PossiblyEvaluated`, `Evaluated`).
// Rust has no variadic generics, so a layer's property set is declared with
// the [`paint_properties!`](crate::paint_properties) macro, which generates
// the four structs (one named field per property, in C++ order) and their
// methods. `get::<P>()` / `get_mut::<P>()` keep the C++ `get<P>()` access by
// tag type. The per-property typedefs (`TransitionableType`,
// `UnevaluatedType`, `EvaluatorType`, `PossiblyEvaluatedType`, `Type`,
// `IsDataDriven`, ...) are the associated items of [`Property`]; the tag
// "base classes" `PaintProperty<T>`, `CrossFadedPaintProperty<T>`,
// `DataDrivenPaintProperty<T, A, U>` are the macros in paint_property.rs.
//
// Interpolation with a float `t` is `mln_util::interpolate::Interpolate::interpolate_f32`
// (C++ `util::interpolate(a, b, float t)`).

use std::cell::RefCell;
use std::fmt::Debug;

use mln_expression::expression::FeatureState;
use mln_tile_id::CanonicalTileID;
use mln_tile_id::geometry_tile_data::GeometryTileFeature;
use mln_util::chrono::{Duration, TimePoint};
use mln_util::constants::DEFAULT_TRANSITION_EASE;
use mln_util::interpolate::Interpolate;
use mln_vec_map::VecSet;
use serde::{Deserialize, Serialize};

use crate::expression::{Dependency, ValueConverter};
use crate::property_value::PropertyValue;
use crate::renderer::possibly_evaluated_property_value::PossiblyEvaluatedPropertyValue;
use crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters;
use crate::renderer::property_evaluator::Evaluator;
use crate::renderer::transition_parameters::TransitionParameters;
use crate::transition_options::TransitionOptions;

/// What `Transitioning<Value>` / `Transitionable<Value>` need from `Value`
/// (`PropertyValue<T>`; later `ColorRampPropertyValue`).
pub trait PropertyValueLike: Clone + Default + Debug {
    type Type;
    fn evaluate<E: Evaluator<Self::Type>>(&self, evaluator: &E) -> E::ResultType;
    fn is_undefined(&self) -> bool;
    fn is_data_driven(&self) -> bool;
    fn get_dependencies(&self) -> Dependency;
    fn get_global_state_refs(&self) -> Option<&VecSet<String>>;
    fn has_data_driven_property_difference(&self, other: &Self) -> bool;
}

impl<T: Clone + Debug + PartialEq> PropertyValueLike for PropertyValue<T> {
    type Type = T;

    fn evaluate<E: Evaluator<T>>(&self, evaluator: &E) -> E::ResultType {
        PropertyValue::evaluate(self, evaluator)
    }

    fn is_undefined(&self) -> bool {
        PropertyValue::is_undefined(self)
    }

    fn is_data_driven(&self) -> bool {
        PropertyValue::is_data_driven(self)
    }

    fn get_dependencies(&self) -> Dependency {
        PropertyValue::get_dependencies(self)
    }

    fn get_global_state_refs(&self) -> Option<&VecSet<String>> {
        PropertyValue::get_global_state_refs(self)
    }

    fn has_data_driven_property_difference(&self, other: &Self) -> bool {
        PropertyValue::has_data_driven_property_difference(self, other)
    }
}

/// `Transitioning<Value>`.
#[derive(Clone, Debug, Default)]
pub struct Transitioning<V> {
    // C++: `mutable std::optional<recursive_wrapper<Transitioning<Value>>>`;
    // `evaluate` is const but drops the prior once the transition is over,
    // which `hasTransition()` (and so the map's continuous repaint) observes.
    prior: RefCell<Option<Box<Transitioning<V>>>>,
    begin: TimePoint,
    end: TimePoint,
    value: V,
}

impl<V: PropertyValueLike> Transitioning<V> {
    /// `explicit Transitioning(Value)`.
    pub fn new(value: V) -> Self {
        Transitioning { prior: RefCell::new(None), begin: TimePoint::default(), end: TimePoint::default(), value }
    }

    /// `Transitioning(Value, Transitioning<Value> prior, const TransitionOptions&, TimePoint now)`.
    pub fn with_prior(value: V, prior: Transitioning<V>, transition: &TransitionOptions, now: TimePoint) -> Self {
        let begin = now + transition.delay.unwrap_or(Duration::zero());
        let end = begin + transition.duration.unwrap_or(Duration::zero());
        Transitioning {
            prior: RefCell::new(if transition.is_defined() { Some(Box::new(prior)) } else { None }),
            begin,
            end,
            value,
        }
    }

    pub fn evaluate<E>(&self, evaluator: &E, now: TimePoint) -> E::ResultType
    where
        E: Evaluator<V::Type>,
        E::ResultType: Interpolate,
    {
        let final_value = self.value.evaluate(evaluator);
        if self.prior.borrow().is_none() {
            // No prior value.
            final_value
        } else if now >= self.end {
            // Transition from prior value is now complete.
            *self.prior.borrow_mut() = None;
            final_value
        } else if self.value.is_data_driven() {
            // Transitions to data-driven properties are not supported.
            // We snap immediately to the data-driven value so that, when we
            // perform layout, we see the data-driven function and can use it to
            // populate vertex buffers.
            *self.prior.borrow_mut() = None;
            final_value
        } else if now < self.begin {
            // Transition hasn't started yet.
            let prior = self.prior.borrow();
            prior.as_ref().unwrap().evaluate(evaluator, now)
        } else {
            // Interpolate between recursively-calculated prior value and final.
            let t: f32 = Duration::float_seconds_div(now - self.begin, self.end - self.begin);
            let prior_value = {
                let prior = self.prior.borrow();
                prior.as_ref().unwrap().evaluate(evaluator, now)
            };
            // util::interpolate(prior, final, static_cast<float>(ease.solve(t, 0.001))):
            // the float-t overload.
            prior_value.interpolate_f32(&final_value, DEFAULT_TRANSITION_EASE.solve(t as f64, 0.001) as f32)
        }
    }

    pub fn has_transition(&self) -> bool {
        self.prior.borrow().is_some()
    }

    pub fn is_transitioning(&self, now: TimePoint) -> bool {
        self.has_transition() && (self.begin <= now) && (now < self.end) && !self.value.is_data_driven()
    }

    pub fn is_undefined(&self) -> bool {
        self.value.is_undefined()
    }

    pub fn get_value(&self) -> &V {
        &self.value
    }

    pub fn get_value_mut(&mut self) -> &mut V {
        &mut self.value
    }
}

/// `Transitionable<Value>`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Transitionable<V> {
    pub value: V,
    pub options: TransitionOptions,
}

impl<V: PropertyValueLike> Transitionable<V> {
    pub fn transition(&self, params: &TransitionParameters, prior: Transitioning<V>) -> Transitioning<V> {
        Transitioning::with_prior(
            self.value.clone(),
            prior,
            &self.options.reverse_merge(&params.transition),
            params.now,
        )
    }
}

/// The member typedefs and statics of a C++ property tag type
/// (`struct BackgroundColor : PaintProperty<Color> { static Color defaultValue(); }`).
pub trait Property: 'static {
    /// `P::Type`.
    type Type: Clone + Debug;
    /// The `Value` of `P::TransitionableType = Transitionable<Value>` and
    /// `P::UnevaluatedType = Transitioning<Value>`.
    type Value: PropertyValueLike<Type = Self::Type>;
    /// `P::PossiblyEvaluatedType`. Paint properties also need it to be
    /// `Interpolate` (transitions); layout values (images, formatted text)
    /// are never interpolated, so the bound is on the paint paths.
    type PossiblyEvaluatedType: Clone + Debug;
    /// `P::EvaluatorType`.
    type EvaluatorType<'a>: Evaluator<Self::Type, ResultType = Self::PossiblyEvaluatedType>;

    const IS_DATA_DRIVEN: bool;
    const IS_OVERRIDABLE: bool;

    /// `P::defaultValue()`.
    fn default_value() -> Self::Type;

    /// `Evaluator(parameters, P::defaultValue())`.
    fn evaluator(parameters: &PropertyEvaluationParameters, default_value: Self::Type) -> Self::EvaluatorType<'_>;

    /// `PossiblyEvaluatedType::isConstant()` (true for plain values), for
    /// `ConstantsMask`.
    fn possibly_evaluated_is_constant(_value: &Self::PossiblyEvaluatedType) -> bool {
        true
    }

    /// `PossiblyEvaluated::getDependencies(const P&)`.
    fn possibly_evaluated_dependencies(_value: &Self::PossiblyEvaluatedType) -> Dependency {
        Dependency::NONE
    }

    /// `PossiblyEvaluated::collectGlobalStateRefs(const P&, ...)`.
    fn possibly_evaluated_global_state_refs(_value: &Self::PossiblyEvaluatedType) -> Option<&VecSet<String>> {
        None
    }

    /// The static `PossiblyEvaluated::evaluate(z, feature, [extra,] value,
    /// defaultValue[, extra])` overload that `evaluate<P>(z, feature, ...)`
    /// picks for this property's `PossiblyEvaluatedType`.
    fn evaluate_possibly_evaluated(
        value: &Self::PossiblyEvaluatedType,
        z: f32,
        feature: &dyn GeometryTileFeature,
        extra: FeatureEvaluationExtra<'_>,
        default_value: Self::Type,
    ) -> Self::Type;
}

/// The extra arguments of the `PossiblyEvaluated::evaluate<P>(z, feature, ...)`
/// overloads.
#[derive(Clone, Copy)]
pub enum FeatureEvaluationExtra<'a> {
    None,
    Canonical(&'a CanonicalTileID),
    State(&'a FeatureState),
    Images(&'a VecSet<String>),
    ImagesCanonical(&'a VecSet<String>, &'a CanonicalTileID),
}

/// `template <class T> static T evaluate(float, const GeometryTileFeature&, [const
/// CanonicalTileID&,] const T& t, const T&) { return t; }`. C++ has no
/// overload for a plain value with feature state or images: such a call
/// does not compile, so it does not happen here either.
pub fn evaluate_constant<T: Clone>(value: &T, extra: FeatureEvaluationExtra<'_>) -> T {
    match extra {
        FeatureEvaluationExtra::None | FeatureEvaluationExtra::Canonical(_) => value.clone(),
        _ => unreachable!("C++ PossiblyEvaluated::evaluate has no overload for a constant with this argument"),
    }
}

/// The `PossiblyEvaluatedPropertyValue<T>` overloads of
/// `PossiblyEvaluated::evaluate`.
pub fn evaluate_possibly_evaluated_property_value<T: Clone + ValueConverter>(
    value: &PossiblyEvaluatedPropertyValue<T>,
    z: f32,
    feature: &dyn GeometryTileFeature,
    extra: FeatureEvaluationExtra<'_>,
    default_value: T,
) -> T {
    match value {
        PossiblyEvaluatedPropertyValue::Constant(t) => t.clone(),
        PossiblyEvaluatedPropertyValue::Expression(t) => match extra {
            FeatureEvaluationExtra::None => t.evaluate_zoom_feature(z, feature, default_value),
            FeatureEvaluationExtra::Canonical(canonical) => {
                t.evaluate_zoom_feature_canonical(z, feature, canonical, default_value)
            }
            FeatureEvaluationExtra::State(state) => t.evaluate_zoom_feature_state(z, feature, state, default_value),
            FeatureEvaluationExtra::Images(images) => t.evaluate_zoom_feature_images(z, feature, images, default_value),
            FeatureEvaluationExtra::ImagesCanonical(images, canonical) => {
                t.evaluate_zoom_feature_images_canonical(z, feature, images, canonical, default_value)
            }
        },
    }
}

/// The cross-faded kinds (`Faded<T>`, `PossiblyEvaluatedPropertyValue<Faded<T>>`)
/// match no `PossiblyEvaluated::evaluate` overload in C++ (the value's `T`
/// and the default's `T` differ), so `evaluate<P>` does not compile for
/// them; the renderer evaluates them on the value
/// (`PossiblyEvaluatedFadedPropertyValue::evaluate`).
pub fn evaluate_cross_faded<T>() -> T {
    unreachable!("C++ PossiblyEvaluated::evaluate<P> does not compile for a cross-faded property")
}

/// `IndexedTuple::get<P>()`: field access by property tag type. The
/// generated structs also have inherent `get::<P>()` / `get_mut::<P>()`.
pub trait Field<P> {
    type Output;
    fn field(&self) -> &Self::Output;
    fn field_mut(&mut self) -> &mut Self::Output;
}

/// `Unevaluated::maybeEvaluate<P>`: evaluate the property if necessary, or
/// produce a copy of the previous value if appropriate.
pub fn maybe_evaluate<P: Property>(
    property: &Transitioning<P::Value>,
    parameters: &PropertyEvaluationParameters,
    old_result: &P::PossiblyEvaluatedType,
) -> P::PossiblyEvaluatedType
where
    P::PossiblyEvaluatedType: Interpolate,
{
    let dependencies = property.get_value().get_dependencies();
    let need_evaluate = parameters.layer_changed
        || parameters.has_crossfade
        || property.has_transition()
        || (parameters.zoom_changed && dependencies.intersects(Dependency::ZOOM))
        || (parameters.global_state_changed
            && dependencies.intersects(Dependency::GLOBAL_STATE)
            && crate::expression::global_state_refs_intersect(
                property.get_value().get_global_state_refs(),
                parameters.changed_global_state_keys.as_deref(),
            ));
    if need_evaluate {
        property.evaluate(&P::evaluator(parameters, P::default_value()), parameters.now)
    } else {
        old_result.clone()
    }
}

/// Declares a paint-style property set: `Properties<Ps...>` where every `P`
/// is a paint (or light) property. Generates `mod $module` with the
/// `Transitionable`, `Unevaluated`, `PossiblyEvaluated` and `Evaluated`
/// structs and the C++ member functions.
///
/// This is what a Rust port of scripts/generate-style-code.mjs will emit,
/// one invocation per layer type.
#[macro_export]
macro_rules! paint_properties {
    ($(#[$meta:meta])* $vis:vis mod $module:ident { $($field:ident: $prop:ty),+ $(,)? }) => {
        $(#[$meta])*
        $vis mod $module {
            #[allow(unused_imports)]
            use super::*;
            #[allow(unused_imports)]
            use $crate::properties::{Field, Property, PropertyValueLike};

            #[derive(Clone, Debug, Default, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
            pub struct Transitionable {
                $(pub $field: $crate::properties::Transitionable<<$prop as Property>::Value>,)+
            }

            #[derive(Clone, Debug, Default)]
            pub struct Unevaluated {
                $(pub $field: $crate::properties::Transitioning<<$prop as Property>::Value>,)+
            }

            #[derive(Clone, Debug, Default, ::serde::Serialize, ::serde::Deserialize)]
            pub struct PossiblyEvaluated {
                $(pub $field: <$prop as Property>::PossiblyEvaluatedType,)+
            }

            #[derive(Clone, Debug, Default)]
            pub struct Evaluated {
                $(pub $field: <$prop as Property>::Type,)+
            }

            $crate::__properties_field_impls!(Transitionable; $($field: $prop),+);
            $crate::__properties_field_impls!(Unevaluated; $($field: $prop),+);
            $crate::__properties_field_impls!(PossiblyEvaluated; $($field: $prop),+);
            $crate::__properties_field_impls!(Evaluated; $($field: $prop),+);

            impl Transitionable {
                pub fn transitioned(
                    &self,
                    parameters: &$crate::renderer::transition_parameters::TransitionParameters,
                    prior: Unevaluated,
                ) -> Unevaluated {
                    Unevaluated {
                        $($field: self.$field.transition(parameters, prior.$field),)+
                    }
                }

                pub fn untransitioned(&self) -> Unevaluated {
                    Unevaluated {
                        $($field: $crate::properties::Transitioning::new(self.$field.value.clone()),)+
                    }
                }

                pub fn has_data_driven_property_difference(&self, other: &Transitionable) -> bool {
                    let mut result = false;
                    $(result |= self.$field.value.has_data_driven_property_difference(&other.$field.value);)+
                    result
                }

                pub fn get_dependencies(&self) -> $crate::expression::Dependency {
                    let mut result = $crate::expression::Dependency::NONE;
                    $(result |= self.$field.value.get_dependencies();)+
                    result
                }
            }

            impl Unevaluated {
                pub fn has_transition(&self) -> bool {
                    let mut result = false;
                    $(result |= self.$field.has_transition();)+
                    result
                }

                /// `evaluate(const PropertyEvaluationParameters&)`.
                pub fn evaluate(
                    &self,
                    parameters: &$crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters,
                ) -> PossiblyEvaluated {
                    PossiblyEvaluated {
                        $($field: self.$field.evaluate(
                            &<$prop as Property>::evaluator(parameters, <$prop as Property>::default_value()),
                            parameters.now,
                        ),)+
                    }
                }

                /// `evaluate(const PropertyEvaluationParameters&, const PossiblyEvaluated& previous)`:
                /// optionally evaluate each property or produce a copy of the
                /// previous value, if appropriate.
                pub fn evaluate_with_previous(
                    &self,
                    parameters: &$crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters,
                    previous: &PossiblyEvaluated,
                ) -> PossiblyEvaluated {
                    PossiblyEvaluated {
                        $($field: $crate::properties::maybe_evaluate::<$prop>(&self.$field, parameters, &previous.$field),)+
                    }
                }

                /// Get the combined dependencies of any contained expressions
                pub fn get_dependencies(&self) -> $crate::expression::Dependency {
                    let mut result = $crate::expression::Dependency::NONE;
                    $(result |= self.$field.get_value().get_dependencies();)+
                    result
                }

                pub fn collect_global_state_refs(&self, refs: &mut mln_vec_map::VecSet<String>) {
                    $(if let Some(property_refs) = self.$field.get_value().get_global_state_refs() {
                        refs.extend(property_refs.iter().cloned());
                    })+
                }

                // TODO(port): updateGPUExpressions (gfx::GPUExpression) and
                // stringify (rapidjson writer).
            }

            impl PossiblyEvaluated {
                /// Extract dependencies from possibly-evaluated properties which may have an expression.
                pub fn get_dependencies(&self) -> $crate::expression::Dependency {
                    let mut result = $crate::expression::Dependency::NONE;
                    $(result |= <$prop as Property>::possibly_evaluated_dependencies(&self.$field);)+
                    result
                }

                pub fn collect_global_state_refs(&self, refs: &mut mln_vec_map::VecSet<String>) {
                    $(if let Some(property_refs) = <$prop as Property>::possibly_evaluated_global_state_refs(&self.$field) {
                        refs.extend(property_refs.iter().cloned());
                    })+
                }

                /// `ConstantsMask<DataDrivenProperties>::getMask`: bit i is set
                /// when the i-th data-driven property is constant.
                pub fn constants_mask(&self) -> u64 {
                    let mut result = 0u64;
                    let mut _index = 0u32;
                    $(if <$prop as Property>::IS_DATA_DRIVEN {
                        if <$prop as Property>::possibly_evaluated_is_constant(&self.$field) {
                            result |= 1u64 << _index;
                        }
                        _index += 1;
                    })+
                    result
                }

                $crate::__possibly_evaluated_feature_methods!();

                // TODO(port): `Evaluated evaluate(z, feature)` (all properties
                // at once; no caller in the paths ported so far).
            }
        }
    };
}

/// `PossiblyEvaluated::evaluate<P>(z, feature, ...)`, the per-property
/// overloads.
#[doc(hidden)]
#[macro_export]
macro_rules! __possibly_evaluated_feature_methods {
    () => {
        /// `evaluate<P>(float z, const GeometryTileFeature& feature)`.
        pub fn evaluate<P>(
            &self,
            z: f32,
            feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
        ) -> <P as $crate::properties::Property>::Type
        where
            P: $crate::properties::Property,
            Self: $crate::properties::Field<P, Output = <P as $crate::properties::Property>::PossiblyEvaluatedType>,
        {
            P::evaluate_possibly_evaluated(
                <Self as $crate::properties::Field<P>>::field(self),
                z,
                feature,
                $crate::properties::FeatureEvaluationExtra::None,
                P::default_value(),
            )
        }

        /// `evaluate<P>(float z, const GeometryTileFeature&, const CanonicalTileID&)`.
        pub fn evaluate_canonical<P>(
            &self,
            z: f32,
            feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
            canonical: &mln_tile_id::CanonicalTileID,
        ) -> <P as $crate::properties::Property>::Type
        where
            P: $crate::properties::Property,
            Self: $crate::properties::Field<P, Output = <P as $crate::properties::Property>::PossiblyEvaluatedType>,
        {
            P::evaluate_possibly_evaluated(
                <Self as $crate::properties::Field<P>>::field(self),
                z,
                feature,
                $crate::properties::FeatureEvaluationExtra::Canonical(canonical),
                P::default_value(),
            )
        }

        /// `evaluate<P>(float z, const GeometryTileFeature&, const FeatureState&)`.
        pub fn evaluate_state<P>(
            &self,
            z: f32,
            feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
            state: &mln_expression::expression::FeatureState,
        ) -> <P as $crate::properties::Property>::Type
        where
            P: $crate::properties::Property,
            Self: $crate::properties::Field<P, Output = <P as $crate::properties::Property>::PossiblyEvaluatedType>,
        {
            P::evaluate_possibly_evaluated(
                <Self as $crate::properties::Field<P>>::field(self),
                z,
                feature,
                $crate::properties::FeatureEvaluationExtra::State(state),
                P::default_value(),
            )
        }

        /// `evaluate<P>(float z, const GeometryTileFeature&, const std::set<std::string>& availableImages)`.
        pub fn evaluate_images<P>(
            &self,
            z: f32,
            feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
            available_images: &mln_vec_map::VecSet<String>,
        ) -> <P as $crate::properties::Property>::Type
        where
            P: $crate::properties::Property,
            Self: $crate::properties::Field<P, Output = <P as $crate::properties::Property>::PossiblyEvaluatedType>,
        {
            P::evaluate_possibly_evaluated(
                <Self as $crate::properties::Field<P>>::field(self),
                z,
                feature,
                $crate::properties::FeatureEvaluationExtra::Images(available_images),
                P::default_value(),
            )
        }

        /// `evaluate<P>(float z, const GeometryTileFeature&, availableImages, const CanonicalTileID&)`.
        pub fn evaluate_images_canonical<P>(
            &self,
            z: f32,
            feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
            available_images: &mln_vec_map::VecSet<String>,
            canonical: &mln_tile_id::CanonicalTileID,
        ) -> <P as $crate::properties::Property>::Type
        where
            P: $crate::properties::Property,
            Self: $crate::properties::Field<P, Output = <P as $crate::properties::Property>::PossiblyEvaluatedType>,
        {
            P::evaluate_possibly_evaluated(
                <Self as $crate::properties::Field<P>>::field(self),
                z,
                feature,
                $crate::properties::FeatureEvaluationExtra::ImagesCanonical(available_images, canonical),
                P::default_value(),
            )
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __properties_field_impls {
    (Transitionable; $($field:ident: $prop:ty),+) => {
        $crate::__properties_field_impls!(@impl Transitionable;
            $($field: $prop => $crate::properties::Transitionable<<$prop as $crate::properties::Property>::Value>),+);
    };
    (Unevaluated; $($field:ident: $prop:ty),+) => {
        $crate::__properties_field_impls!(@impl Unevaluated;
            $($field: $prop => $crate::properties::Transitioning<<$prop as $crate::properties::Property>::Value>),+);
    };
    (PossiblyEvaluated; $($field:ident: $prop:ty),+) => {
        $crate::__properties_field_impls!(@impl PossiblyEvaluated;
            $($field: $prop => <$prop as $crate::properties::Property>::PossiblyEvaluatedType),+);
    };
    (Evaluated; $($field:ident: $prop:ty),+) => {
        $crate::__properties_field_impls!(@impl Evaluated;
            $($field: $prop => <$prop as $crate::properties::Property>::Type),+);
    };
    (@impl $s:ident; $($field:ident: $prop:ty => $out:ty),+) => {
        $(impl $crate::properties::Field<$prop> for $s {
            type Output = $out;
            fn field(&self) -> &Self::Output {
                &self.$field
            }
            fn field_mut(&mut self) -> &mut Self::Output {
                &mut self.$field
            }
        })+

        impl $s {
            /// `get<P>()`.
            pub fn get<P>(&self) -> &<Self as $crate::properties::Field<P>>::Output
            where
                Self: $crate::properties::Field<P>,
            {
                <Self as $crate::properties::Field<P>>::field(self)
            }

            /// `get<P>()` (mutable).
            pub fn get_mut<P>(&mut self) -> &mut <Self as $crate::properties::Field<P>>::Output
            where
                Self: $crate::properties::Field<P>,
            {
                <Self as $crate::properties::Field<P>>::field_mut(self)
            }
        }
    };
}

/// What an overridable property's `hasOverride(const T& t)` reads
/// (`!!t.textColor`): C++ calls it on both `expression::FormattedSection` and
/// `expression::FormatExpressionSection`.
pub trait OverridableSection {
    /// `!!section.textColor`.
    fn has_text_color(&self) -> bool;
}

impl OverridableSection for mln_expression::formatted::FormattedSection {
    fn has_text_color(&self) -> bool {
        self.text_color.is_some()
    }
}

impl OverridableSection for mln_expression::format_expression::FormatExpressionSection {
    fn has_text_color(&self) -> bool {
        self.text_color.is_some()
    }
}
