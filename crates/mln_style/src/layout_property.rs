// Ports src/mln/style/layout_property.hpp: `LayoutProperty<T>` and
// `DataDrivenLayoutProperty<T>`, plus the layout half of the C++
// `Properties<Ps...>` template (properties.hpp) as the
// [`layout_properties!`](crate::layout_properties) macro.
//
// Layout properties have no `Transitionable` (C++: `TransitionableType =
// std::nullptr_t`): the unevaluated value is the `PropertyValue<T>` itself.
// Every C++ layout property tag also has `static constexpr const char*
// name()`; the macros give the tag an inherent `name()`.

/// `LayoutProperty<T>`.
#[macro_export]
macro_rules! layout_property {
    ($(#[$meta:meta])* $vis:vis struct $name:ident: $t:ty = $default:expr, name = $prop_name:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        $vis struct $name;

        impl $name {
            /// `static constexpr const char *name()`.
            pub const fn name() -> &'static str {
                $prop_name
            }
        }

        impl $crate::properties::Property for $name {
            type Type = $t;
            type Value = $crate::property_value::PropertyValue<$t>;
            type PossiblyEvaluatedType = $t;
            type EvaluatorType<'a> = $crate::renderer::property_evaluator::PropertyEvaluator<'a, $t>;
            const IS_DATA_DRIVEN: bool = false;
            const IS_OVERRIDABLE: bool = false;

            fn default_value() -> $t {
                $default
            }

            fn evaluator(
                parameters: &$crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters,
                default_value: $t,
            ) -> Self::EvaluatorType<'_> {
                $crate::renderer::property_evaluator::PropertyEvaluator::new(parameters, default_value)
            }

            $crate::__constant_feature_evaluation!($t);
        }
    };
}

/// `DataDrivenLayoutProperty<T>`.
#[macro_export]
macro_rules! data_driven_layout_property {
    ($(#[$meta:meta])* $vis:vis struct $name:ident: $t:ty = $default:expr, name = $prop_name:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        $vis struct $name;

        impl $name {
            /// `static constexpr const char *name()`.
            pub const fn name() -> &'static str {
                $prop_name
            }
        }

        impl $crate::properties::Property for $name {
            type Type = $t;
            type Value = $crate::property_value::PropertyValue<$t>;
            type PossiblyEvaluatedType =
                $crate::renderer::possibly_evaluated_property_value::PossiblyEvaluatedPropertyValue<$t>;
            type EvaluatorType<'a> =
                $crate::renderer::data_driven_property_evaluator::DataDrivenPropertyEvaluator<'a, $t, false>;
            const IS_DATA_DRIVEN: bool = true;
            const IS_OVERRIDABLE: bool = false;

            fn default_value() -> $t {
                $default
            }

            fn evaluator(
                parameters: &$crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters,
                default_value: $t,
            ) -> Self::EvaluatorType<'_> {
                $crate::renderer::data_driven_property_evaluator::DataDrivenPropertyEvaluator::new(
                    parameters,
                    default_value,
                )
            }

            $crate::__data_driven_possibly_evaluated_hooks!();
            $crate::__data_driven_feature_evaluation!($t);
        }
    };
}

/// Declares a layout property set: `Properties<Ps...>` where every `P` is a
/// layout property. Generates `mod $module` with the `Unevaluated` (a
/// `PropertyValue<T>` per property), `PossiblyEvaluated` and `Evaluated`
/// structs and the C++ member functions layout properties use.
#[macro_export]
macro_rules! layout_properties {
    ($(#[$meta:meta])* $vis:vis mod $module:ident { $($field:ident: $prop:ty),+ $(,)? }) => {
        $(#[$meta])*
        $vis mod $module {
            #[allow(unused_imports)]
            use super::*;
            #[allow(unused_imports)]
            use $crate::properties::{Field, Property, PropertyValueLike};

            #[derive(Clone, Debug, Default, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
            pub struct Unevaluated {
                $(pub $field: <$prop as Property>::Value,)+
            }

            #[derive(Clone, Debug, Default, ::serde::Serialize, ::serde::Deserialize)]
            pub struct PossiblyEvaluated {
                $(pub $field: <$prop as Property>::PossiblyEvaluatedType,)+
            }

            #[derive(Clone, Debug, Default)]
            pub struct Evaluated {
                $(pub $field: <$prop as Property>::Type,)+
            }

            $crate::__properties_field_impls!(@impl Unevaluated;
                $($field: $prop => <$prop as Property>::Value),+);
            $crate::__properties_field_impls!(PossiblyEvaluated; $($field: $prop),+);
            $crate::__properties_field_impls!(Evaluated; $($field: $prop),+);

            impl Unevaluated {
                /// `evaluate(const PropertyEvaluationParameters&)`.
                pub fn evaluate(
                    &self,
                    parameters: &$crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters,
                ) -> PossiblyEvaluated {
                    PossiblyEvaluated {
                        $($field: self.$field.evaluate(
                            &<$prop as Property>::evaluator(parameters, <$prop as Property>::default_value()),
                        ),)+
                    }
                }

                /// `constantsMask()`: bit i is set when the i-th data-driven
                /// property is a constant.
                pub fn constants_mask(&self) -> u64 {
                    let mut result = 0u64;
                    let mut _index = 0u32;
                    $(if <$prop as Property>::IS_DATA_DRIVEN {
                        if self.$field.is_constant() {
                            result |= 1u64 << _index;
                        }
                        _index += 1;
                    })+
                    result
                }

                /// Get the combined dependencies of any contained expressions
                pub fn get_dependencies(&self) -> $crate::expression::Dependency {
                    let mut result = $crate::expression::Dependency::NONE;
                    $(result |= self.$field.get_dependencies();)+
                    result
                }

                pub fn collect_global_state_refs(&self, refs: &mut std::collections::BTreeSet<String>) {
                    $(if let Some(property_refs) = self.$field.get_global_state_refs() {
                        refs.extend(property_refs.iter().cloned());
                    })+
                }

                /// `stringify(writer)` (conversion/stringify.hpp; only
                /// stringifyLayout calls it, and nothing in the C++ core calls
                /// that). gtests-data: ported for Stringify.Layout.
                pub fn stringify(&self, writer: &mut $crate::conversion::stringify::Writer) {
                    writer.start_object();
                    $($crate::conversion::stringify::stringify_property(writer, <$prop>::name(), &self.$field);)+
                    writer.end_object();
                }

                // TODO(port): updateGPUExpressions (gfx::GPUExpression).
            }

            impl PossiblyEvaluated {
                /// Extract dependencies from possibly-evaluated properties which may have an expression.
                pub fn get_dependencies(&self) -> $crate::expression::Dependency {
                    let mut result = $crate::expression::Dependency::NONE;
                    $(result |= <$prop as Property>::possibly_evaluated_dependencies(&self.$field);)+
                    result
                }

                pub fn collect_global_state_refs(&self, refs: &mut std::collections::BTreeSet<String>) {
                    $(if let Some(property_refs) = <$prop as Property>::possibly_evaluated_global_state_refs(&self.$field) {
                        refs.extend(property_refs.iter().cloned());
                    })+
                }

                /// `ConstantsMask<DataDrivenProperties>::getMask`.
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

                /// `Evaluated evaluate(float z, const GeometryTileFeature&,
                /// const CanonicalTileID&)`: every property at once (symbol
                /// layout; added by `symbols`).
                pub fn evaluate_all(
                    &self,
                    z: f32,
                    feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
                    canonical: &mln_tile_id::CanonicalTileID,
                ) -> Evaluated {
                    Evaluated {
                        $($field: <$prop as Property>::evaluate_possibly_evaluated(
                            &self.$field,
                            z,
                            feature,
                            $crate::properties::FeatureEvaluationExtra::Canonical(canonical),
                            <$prop as Property>::default_value(),
                        ),)+
                    }
                }
            }
        }
    };
}
