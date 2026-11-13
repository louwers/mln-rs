// C++ declares a property as `struct BackgroundColor : PaintProperty<Color> {
// static Color defaultValue() { ... } };`. The Rust tag is a unit struct whose
// `Property` impl these macros write:
//
//     paint_property!(pub struct BackgroundColor: Color = Color::black());
//
// The `A`/`U` (attribute, uniform) template arguments of the data-driven
// kinds are not carried: they name gfx attribute bindings, which the
// renderer looks up by property.

macro_rules! paint_property {
    ($(#[$meta:meta])* $vis:vis struct $name:ident: $t:ty = $default:expr) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        $vis struct $name;

        impl $crate::properties::Property for $name {
            type Type = $t;
            type Value = $crate::property_value::PropertyValue<$t>;
            type PossiblyEvaluatedType = $t;
            type EvaluatorType<'a> = $crate::renderer::property_evaluator::PropertyEvaluator<'a, $t>;
            const IS_DATA_DRIVEN: bool = false;

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
pub(crate) use paint_property;

macro_rules! cross_faded_paint_property {
    ($(#[$meta:meta])* $vis:vis struct $name:ident: $t:ty = $default:expr) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        $vis struct $name;

        impl $crate::properties::Property for $name {
            type Type = $t;
            type Value = $crate::property_value::PropertyValue<$t>;
            type PossiblyEvaluatedType = $crate::renderer::cross_faded_property_evaluator::Faded<$t>;
            type EvaluatorType<'a> =
                $crate::renderer::cross_faded_property_evaluator::CrossFadedPropertyEvaluator<'a, $t>;
            const IS_DATA_DRIVEN: bool = false;

            fn default_value() -> $t {
                $default
            }

            fn evaluator(
                parameters: &$crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters,
                default_value: $t,
            ) -> Self::EvaluatorType<'_> {
                $crate::renderer::cross_faded_property_evaluator::CrossFadedPropertyEvaluator::new(
                    parameters,
                    default_value,
                )
            }

            $crate::__cross_faded_feature_evaluation!($t);
        }
    };
}
pub(crate) use cross_faded_paint_property;

/// Declares a data-driven paint property. `use_integer_zoom = true` replaces
/// the evaluator with `DataDrivenPropertyEvaluator<T, true>` (for
/// line-floor-width).
macro_rules! data_driven_paint_property {
    ($(#[$meta:meta])* $vis:vis struct $name:ident: $t:ty = $default:expr
        $(, use_integer_zoom = $int_zoom:literal)?) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        $vis struct $name;

        impl $crate::properties::Property for $name {
            type Type = $t;
            type Value = $crate::property_value::PropertyValue<$t>;
            type PossiblyEvaluatedType =
                $crate::renderer::possibly_evaluated_property_value::PossiblyEvaluatedPropertyValue<$t>;
            type EvaluatorType<'a> =
                $crate::renderer::data_driven_property_evaluator::DataDrivenPropertyEvaluator<
                    'a,
                    $t,
                    { false $(|| $int_zoom)? },
                >;
            const IS_DATA_DRIVEN: bool = true;

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
pub(crate) use data_driven_paint_property;

macro_rules! cross_faded_data_driven_paint_property {
    ($(#[$meta:meta])* $vis:vis struct $name:ident: $t:ty = $default:expr) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        $vis struct $name;

        impl $crate::properties::Property for $name {
            type Type = $t;
            type Value = $crate::property_value::PropertyValue<$t>;
            type PossiblyEvaluatedType =
                $crate::renderer::possibly_evaluated_property_value::PossiblyEvaluatedFadedPropertyValue<$t>;
            type EvaluatorType<'a> =
                $crate::renderer::data_driven_property_evaluator::DataDrivenFadedPropertyEvaluator<'a, $t>;
            const IS_DATA_DRIVEN: bool = true;

            fn default_value() -> $t {
                $default
            }

            fn evaluator(
                parameters: &$crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters,
                default_value: $t,
            ) -> Self::EvaluatorType<'_> {
                $crate::renderer::data_driven_property_evaluator::DataDrivenFadedPropertyEvaluator::new(
                    parameters,
                    default_value,
                )
            }

            $crate::__data_driven_possibly_evaluated_hooks!();
            $crate::__cross_faded_feature_evaluation!($t);
        }
    };
}
pub(crate) use cross_faded_data_driven_paint_property;

/// Declares a color ramp paint property, the special case for
/// heatmap-color, line-gradient and color-relief-color.
macro_rules! color_ramp_property {
    ($(#[$meta:meta])* $vis:vis struct $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
        $vis struct $name;

        impl $crate::properties::Property for $name {
            type Type = mln_util::Color;
            type Value = $crate::color_ramp_property_value::ColorRampPropertyValue;
            type PossiblyEvaluatedType = mln_util::Color;
            type EvaluatorType<'a> = $crate::renderer::property_evaluator::PropertyEvaluator<'a, mln_util::Color>;
            const IS_DATA_DRIVEN: bool = false;

            fn default_value() -> mln_util::Color {
                mln_util::Color::default()
            }

            fn evaluator(
                parameters: &$crate::renderer::property_evaluation_parameters::PropertyEvaluationParameters,
                default_value: mln_util::Color,
            ) -> Self::EvaluatorType<'_> {
                $crate::renderer::property_evaluator::PropertyEvaluator::new(parameters, default_value)
            }

            $crate::__constant_feature_evaluation!(mln_util::Color);
        }
    };
}
pub(crate) use color_ramp_property;

/// The `PossiblyEvaluated` overloads that pick out a
/// `PossiblyEvaluatedPropertyValue` (`isConstant`, `getDependencies`,
/// `collectGlobalStateRefs`).
macro_rules! __data_driven_possibly_evaluated_hooks {
    () => {
        fn possibly_evaluated_is_constant(value: &Self::PossiblyEvaluatedType) -> bool {
            value.is_constant()
        }

        fn possibly_evaluated_dependencies(value: &Self::PossiblyEvaluatedType) -> $crate::expression::Dependency {
            value.get_dependencies()
        }

        fn possibly_evaluated_global_state_refs(
            value: &Self::PossiblyEvaluatedType,
        ) -> Option<&mln_vec_map::VecSet<String>> {
            value.get_global_state_refs()
        }
    };
}
pub(crate) use __data_driven_possibly_evaluated_hooks;

/// Implements `Property::evaluate_possibly_evaluated` for a plain
/// `PossiblyEvaluatedType`.
macro_rules! __constant_feature_evaluation {
    ($t:ty) => {
        fn evaluate_possibly_evaluated(
            value: &Self::PossiblyEvaluatedType,
            _z: f32,
            _feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
            extra: $crate::properties::FeatureEvaluationExtra<'_>,
            _default_value: $t,
        ) -> $t {
            $crate::properties::evaluate_constant(value, extra)
        }
    };
}
pub(crate) use __constant_feature_evaluation;

/// Implements `Property::evaluate_possibly_evaluated` for a
/// `PossiblyEvaluatedPropertyValue<T>`.
macro_rules! __data_driven_feature_evaluation {
    ($t:ty) => {
        fn evaluate_possibly_evaluated(
            value: &Self::PossiblyEvaluatedType,
            z: f32,
            feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
            extra: $crate::properties::FeatureEvaluationExtra<'_>,
            default_value: $t,
        ) -> $t {
            $crate::properties::evaluate_possibly_evaluated_property_value(value, z, feature, extra, default_value)
        }
    };
}
pub(crate) use __data_driven_feature_evaluation;

/// Implements `Property::evaluate_possibly_evaluated` for the cross-faded
/// kinds.
macro_rules! __cross_faded_feature_evaluation {
    ($t:ty) => {
        fn evaluate_possibly_evaluated(
            _value: &Self::PossiblyEvaluatedType,
            _z: f32,
            _feature: &dyn mln_tile_id::geometry_tile_data::GeometryTileFeature,
            _extra: $crate::properties::FeatureEvaluationExtra<'_>,
            _default_value: $t,
        ) -> $t {
            $crate::properties::evaluate_cross_faded()
        }
    };
}
pub(crate) use __cross_faded_feature_evaluation;
