//! A property set's values at each stage: as the style sets them
//! (`Transitionable`), transitioning from earlier values (`Unevaluated`), and
//! evaluated for a frame (`PossiblyEvaluated`, an [`Evaluated`] per
//! property).
//!
//! `properties!` declares a set: one named field per
//! property, with its type, its default and how it evaluates.

use std::fmt::Debug;
use std::time::Duration;

use crate::constants::DEFAULT_TRANSITION_EASE;
use crate::expression::{Dependency, global_state_refs_intersect};
use crate::interpolate::Interpolate;
use crate::property::evaluated::{Evaluated, Evaluation};
use crate::property::evaluation::PropertyEvaluationParameters;
use crate::property::transition::{GlobalStateChange, TransitionOptions, TransitionParameters};
use crate::property::value::StyleValue;
use crate::time::Instant;

/// A value, and the values it transitions from.
#[derive(Clone, Debug, Default)]
pub struct Transitioning<V> {
    /// The value transitioned from, until the transition ends: evaluating
    /// after that drops it.
    prior: Option<Box<Transitioning<V>>>,
    /// When the transition from `prior` was set up; `None` without a clock,
    /// where it has already ended.
    start: Option<Instant>,
    /// How long after `start` the transition begins.
    delay: Duration,
    /// How long it lasts once it has begun.
    duration: Duration,
    value: V,
}

/// Where a transition stands at a time.
enum Stage {
    /// Its delay hasn't passed yet.
    Delayed,
    /// It is this share of the way from the prior value, in [0, 1).
    Running(f32),
    Ended,
}

impl<V: StyleValue> Transitioning<V> {
    #[must_use]
    pub fn new(value: V) -> Self {
        Transitioning { prior: None, start: None, delay: Duration::ZERO, duration: Duration::ZERO, value }
    }

    /// A transition from `prior` that starts at `now`, or one that has
    /// already ended for `None`.
    pub(crate) fn with_prior(
        value: V,
        prior: Transitioning<V>,
        transition: &TransitionOptions,
        now: Option<Instant>,
    ) -> Self {
        Transitioning {
            prior: if transition.is_defined() { Some(Box::new(prior)) } else { None },
            start: now,
            delay: transition.delay.unwrap_or_default(),
            duration: transition.duration.unwrap_or_default(),
            value,
        }
    }

    fn stage(&self, now: Option<Instant>) -> Stage {
        let (Some(now), Some(start)) = (now, self.start) else {
            return Stage::Ended;
        };
        let Some(running) = now.saturating_duration_since(start).checked_sub(self.delay) else {
            return Stage::Delayed;
        };
        if running < self.duration { Stage::Running(running.div_duration_f32(self.duration)) } else { Stage::Ended }
    }

    /// The value at `parameters.now`, as `evaluation` says, with `default`
    /// for an undefined value. A transition that has ended, or that ends in
    /// a data-driven value, drops the value it came from.
    ///
    /// Not inlined: the property sets call these once
    /// per property, so inlining copies them into every call.
    #[inline(never)]
    pub fn evaluate(
        &mut self,
        evaluation: Evaluation<V::Type>,
        parameters: &PropertyEvaluationParameters,
        default: V::Type,
    ) -> Evaluated<V::Type>
    where
        V::Type: Interpolate,
    {
        let final_value = self.value.evaluate(evaluation, parameters, default.clone());
        let stage = self.stage(parameters.now);
        let Some(prior) = self.prior.as_deref_mut() else {
            return final_value;
        };
        if matches!(stage, Stage::Ended) || self.value.is_data_driven() {
            // Transitions to data-driven properties are not supported: snap
            // to the data-driven value, so that layout sees the expression
            // and populates vertex buffers from it.
            self.prior = None;
            return final_value;
        }
        let prior_value = prior.evaluate(evaluation, parameters, default);
        match stage {
            Stage::Running(t) => {
                prior_value.interpolate_f32(&final_value, DEFAULT_TRANSITION_EASE.solve(f64::from(t), 0.001) as f32)
            }
            Stage::Delayed | Stage::Ended => prior_value,
        }
    }

    pub fn has_transition(&self) -> bool {
        self.prior.is_some()
    }

    pub fn is_undefined(&self) -> bool {
        self.value.is_undefined()
    }

    /// Keeps every step that reads a key changed by `change` on the state from
    /// before it, so that a new transition starts from what this one shows.
    pub(crate) fn retain_prior_global_state(&mut self, change: &GlobalStateChange) {
        self.value.retain_prior_global_state(change);
        if let Some(prior) = &mut self.prior {
            prior.retain_prior_global_state(change);
        }
    }

    pub fn get_value(&self) -> &V {
        &self.value
    }

    /// Evaluates the value if it may have changed since `previous`, or
    /// copies `previous`.
    #[inline(never)]
    pub fn evaluate_with_previous(
        &mut self,
        evaluation: Evaluation<V::Type>,
        parameters: &PropertyEvaluationParameters,
        default: V::Type,
        previous: &Evaluated<V::Type>,
    ) -> Evaluated<V::Type>
    where
        V::Type: Interpolate,
    {
        let dependencies = self.value.get_dependencies();
        let need_evaluate = parameters.layer_changed
            || parameters.has_crossfade
            || self.has_transition()
            || (parameters.zoom_changed && dependencies.intersects(Dependency::ZOOM))
            || (parameters.global_state_changed
                && dependencies.intersects(Dependency::GLOBAL_STATE)
                && global_state_refs_intersect(
                    self.value.get_global_state_refs(),
                    parameters.changed_global_state_keys.as_deref(),
                ));
        if need_evaluate { self.evaluate(evaluation, parameters, default) } else { previous.clone() }
    }
}

/// A value as the style sets it, and how it transitions.
#[derive(Clone, Debug, Default, PartialEq, mln_wire::Wire)]
pub struct Transitionable<V> {
    pub value: V,
    pub(crate) options: TransitionOptions,
}

impl<V: StyleValue> Transitionable<V> {
    /// A value that transitions with the style's default transition.
    #[must_use]
    pub fn new(value: V) -> Self {
        Transitionable { value, options: TransitionOptions::default() }
    }

    /// The value, in transition from `prior` as `params` and the value's
    /// own options say.
    #[inline(never)]
    pub fn transition(&self, params: &TransitionParameters, mut prior: Transitioning<V>) -> Transitioning<V> {
        if let Some(change) = &params.global_state_change {
            prior.retain_prior_global_state(change);
            if change.state_only && !self.value.transitions_on_global_state_change(Some(&change.changed_keys)) {
                return prior;
            }
        }
        Transitioning::with_prior(
            self.value.clone(),
            prior,
            &self.options.reverse_merge(&params.transition),
            params.now,
        )
    }
}

/// The style value type of a property kind.
#[doc(hidden)]
#[macro_export]
macro_rules! __property_value {
    (color_ramp, $t:ty) => {
        $crate::property::color_ramp::ColorRampPropertyValue
    };
    ($kind:ident, $t:ty) => {
        $crate::property::value::PropertyValue<$t>
    };
}

/// How a property kind evaluates.
#[doc(hidden)]
#[macro_export]
macro_rules! __property_evaluation {
    (integer_zoom) => {
        $crate::property::evaluated::Evaluated::integer_zoom
    };
    (cross_faded) => {
        $crate::property::evaluated::Evaluated::cross_faded
    };
    (cross_faded_data_driven) => {
        $crate::property::evaluated::Evaluated::cross_faded_per_feature
    };
    (cross_faded_or_data_driven) => {
        $crate::property::evaluated::Evaluated::cross_faded_or_per_feature
    };
    ($kind:ident) => {
        $crate::property::evaluated::Evaluated::plain
    };
}

/// Whether a property kind may differ by feature.
#[doc(hidden)]
#[macro_export]
macro_rules! __property_data_driven {
    (data_driven) => {
        true
    };
    (integer_zoom) => {
        true
    };
    (cross_faded_data_driven) => {
        true
    };
    (cross_faded_or_data_driven) => {
        true
    };
    ($kind:ident) => {
        false
    };
}

/// Declares a property set as `mod $module`, one named field per property,
/// followed by the paths its types and defaults need:
///
/// ```ignore
/// properties!(
///     pub mod FillPaintProperties: paint {
///         fill_antialias: constant bool = true,
///         fill_color: data_driven Color = Color::black(),
///     }
///     use crate::color::Color;
/// );
/// ```
///
/// A property's kind is its style-spec property type: `constant`,
/// `data_driven`, `cross_faded`, `cross_faded_data_driven` or `color_ramp`,
/// or `integer_zoom` for a data-driven property that evaluates at the integer
/// zoom below. `cross_faded_data_driven` is the patterns' kind;
/// `cross_faded_or_data_driven`, `line-dasharray`'s, cross-fades a value that
/// doesn't read the feature as `cross_faded` does. A `paint` set transitions; a `layout` set doesn't.
///
/// A paint set's `derived { field: kind T = default => source, }` values
/// aren't style properties: each takes `source`'s value, and transitions with
/// the style's default transition.
#[macro_export]
macro_rules! properties {
    ($(#[$meta:meta])* $vis:vis mod $module:ident: paint {
        $($field:ident: $kind:ident $t:ty = $default:expr),+ $(,)?
    } $(derived {
        $($dfield:ident: $dkind:ident $dt:ty = $ddefault:expr => $source:ident),+ $(,)?
    })? $(use $($import:path),+;)?) => {
        $(#[$meta])*
        $vis mod $module {
            $($(use $import;)+)?
            use $crate::property::value::StyleValue as _;

            /// The spec's default of each property.
            pub struct Defaults;

            impl Defaults {
                $(#[must_use]
                pub fn $field() -> $t {
                    $default
                })+
                $($(#[must_use]
                pub fn $dfield() -> $dt {
                    $ddefault
                })+)?
            }

            #[derive(Clone, Debug, Default, PartialEq, $crate::__private::mln_wire::Wire)]
            #[wire(crate = $crate::__private::mln_wire, default)]
            pub struct Transitionable {
                $(pub $field: $crate::property::set::Transitionable<$crate::__property_value!($kind, $t)>,)+
            }

            #[derive(Clone, Debug, Default)]
            pub struct Unevaluated {
                $(pub $field: $crate::property::set::Transitioning<$crate::__property_value!($kind, $t)>,)+
                $($(pub $dfield: $crate::property::set::Transitioning<$crate::__property_value!($dkind, $dt)>,)+)?
            }

            #[derive(Clone, Debug, Default, $crate::__private::mln_wire::Wire)]
            #[wire(crate = $crate::__private::mln_wire, default)]
            pub struct PossiblyEvaluated {
                $(pub $field: $crate::property::evaluated::Evaluated<$t>,)+
                $($(pub $dfield: $crate::property::evaluated::Evaluated<$dt>,)+)?
            }

            impl Transitionable {
                pub fn transitioned(
                    &self,
                    parameters: &$crate::property::transition::TransitionParameters,
                    prior: Unevaluated,
                ) -> Unevaluated {
                    Unevaluated {
                        $($field: self.$field.transition(parameters, prior.$field),)+
                        $($($dfield: $crate::property::set::Transitionable::new(self.$source.value.clone())
                            .transition(parameters, prior.$dfield),)+)?
                    }
                }

                pub fn untransitioned(&self) -> Unevaluated {
                    Unevaluated {
                        $($field: $crate::property::set::Transitioning::new(self.$field.value.clone()),)+
                        $($($dfield: $crate::property::set::Transitioning::new(self.$source.value.clone()),)+)?
                    }
                }

                pub fn has_data_driven_property_difference(&self, other: &Transitionable) -> bool {
                    false $(|| self.$field.value.has_data_driven_property_difference(&other.$field.value))+
                }

                pub fn get_dependencies(&self) -> $crate::expression::Dependency {
                    let mut result = $crate::expression::Dependency::NONE;
                    $(result |= self.$field.value.get_dependencies();)+
                    result
                }
            }

            impl $crate::layer::PaintSet for Transitionable {
                type Evaluated = PossiblyEvaluated;
                type Unevaluated = Unevaluated;

                fn untransitioned(&self) -> Unevaluated {
                    Transitionable::untransitioned(self)
                }

                fn transitioned(
                    &self,
                    parameters: &$crate::property::transition::TransitionParameters,
                    prior: Unevaluated,
                ) -> Unevaluated {
                    Transitionable::transitioned(self, parameters, prior)
                }

                fn evaluate_with_previous(
                    unevaluated: &mut Unevaluated,
                    parameters: &$crate::property::evaluation::PropertyEvaluationParameters,
                    previous: &PossiblyEvaluated,
                ) -> PossiblyEvaluated {
                    unevaluated.evaluate_with_previous(parameters, previous)
                }

                fn has_transition(unevaluated: &Unevaluated) -> bool {
                    unevaluated.has_transition()
                }

                fn dependencies(unevaluated: &Unevaluated) -> $crate::expression::Dependency {
                    unevaluated.get_dependencies()
                }

                fn evaluated_dependencies(evaluated: &PossiblyEvaluated) -> $crate::expression::Dependency {
                    evaluated.get_dependencies()
                }

                fn constants_mask(evaluated: &PossiblyEvaluated) -> u64 {
                    evaluated.constants_mask()
                }

                fn collect_evaluated_global_state_refs(
                    evaluated: &PossiblyEvaluated,
                    refs: &mut $crate::collections::VecSet<String>,
                ) {
                    evaluated.collect_global_state_refs(refs);
                }
            }

            impl Unevaluated {
                pub fn has_transition(&self) -> bool {
                    false $(|| self.$field.has_transition())+ $($(|| self.$dfield.has_transition())+)?
                }

                pub fn evaluate(
                    &mut self,
                    parameters: &$crate::property::evaluation::PropertyEvaluationParameters,
                ) -> PossiblyEvaluated {
                    PossiblyEvaluated {
                        $($field: self.$field.evaluate(
                            $crate::__property_evaluation!($kind),
                            parameters,
                            Defaults::$field(),
                        ),)+
                        $($($dfield: self.$dfield.evaluate(
                            $crate::__property_evaluation!($dkind),
                            parameters,
                            Defaults::$dfield(),
                        ),)+)?
                    }
                }

                /// Evaluates each property, or copies its value from
                /// `previous` where it can't have changed.
                pub fn evaluate_with_previous(
                    &mut self,
                    parameters: &$crate::property::evaluation::PropertyEvaluationParameters,
                    previous: &PossiblyEvaluated,
                ) -> PossiblyEvaluated {
                    PossiblyEvaluated {
                        $($field: self.$field.evaluate_with_previous(
                            $crate::__property_evaluation!($kind),
                            parameters,
                            Defaults::$field(),
                            &previous.$field,
                        ),)+
                        $($($dfield: self.$dfield.evaluate_with_previous(
                            $crate::__property_evaluation!($dkind),
                            parameters,
                            Defaults::$dfield(),
                            &previous.$dfield,
                        ),)+)?
                    }
                }

                /// The combined dependencies of the expressions.
                pub fn get_dependencies(&self) -> $crate::expression::Dependency {
                    let mut result = $crate::expression::Dependency::NONE;
                    $(result |= self.$field.get_value().get_dependencies();)+
                    $($(result |= self.$dfield.get_value().get_dependencies();)+)?
                    result
                }

                pub fn collect_global_state_refs(&self, refs: &mut $crate::collections::VecSet<String>) {
                    $(if let Some(property_refs) = self.$field.get_value().get_global_state_refs() {
                        refs.extend(property_refs.iter().cloned());
                    })+
                    $($(if let Some(property_refs) = self.$dfield.get_value().get_global_state_refs() {
                        refs.extend(property_refs.iter().cloned());
                    })+)?
                }
            }

            $crate::__possibly_evaluated_methods!($($field: $kind),+ $($(, $dfield: $dkind)+)?);
        }
    };
    ($(#[$meta:meta])* $vis:vis mod $module:ident: layout {
        $($field:ident: $kind:ident $t:ty = $default:expr),+ $(,)?
    } $(use $($import:path),+;)?) => {
        $(#[$meta])*
        $vis mod $module {
            $($(use $import;)+)?
            use $crate::property::value::StyleValue as _;

            /// The spec's default of each property.
            pub struct Defaults;

            impl Defaults {
                $(#[must_use]
                pub fn $field() -> $t {
                    $default
                })+
            }

            #[derive(Clone, Debug, Default, PartialEq, $crate::__private::mln_wire::Wire)]
            #[wire(crate = $crate::__private::mln_wire, default)]
            pub struct Unevaluated {
                $(pub $field: $crate::__property_value!($kind, $t),)+
            }

            #[derive(Clone, Debug, Default, $crate::__private::mln_wire::Wire)]
            #[wire(crate = $crate::__private::mln_wire, default)]
            pub struct PossiblyEvaluated {
                $(pub $field: $crate::property::evaluated::Evaluated<$t>,)+
            }

            /// The value of each property for one feature.
            #[derive(Clone, Debug, Default)]
            pub struct Values {
                $(pub $field: $t,)+
            }

            impl Unevaluated {
                pub fn evaluate(
                    &self,
                    parameters: &$crate::property::evaluation::PropertyEvaluationParameters,
                ) -> PossiblyEvaluated {
                    PossiblyEvaluated {
                        $($field: self.$field.evaluate(
                            $crate::__property_evaluation!($kind),
                            parameters,
                            Defaults::$field(),
                        ),)+
                    }
                }

                /// Bit i is set when the i-th data-driven property is a
                /// constant.
                pub fn constants_mask(&self) -> u64 {
                    let mut result = 0u64;
                    let mut _index = 0u32;
                    $(if $crate::__property_data_driven!($kind) {
                        if self.$field.is_constant() {
                            result |= 1u64 << _index;
                        }
                        _index += 1;
                    })+
                    result
                }

                /// The combined dependencies of the expressions.
                pub fn get_dependencies(&self) -> $crate::expression::Dependency {
                    let mut result = $crate::expression::Dependency::NONE;
                    $(result |= self.$field.get_dependencies();)+
                    result
                }

                pub fn collect_global_state_refs(&self, refs: &mut $crate::collections::VecSet<String>) {
                    $(if let Some(property_refs) = self.$field.get_global_state_refs() {
                        refs.extend(property_refs.iter().cloned());
                    })+
                }
            }

            impl $crate::layer::LayoutSet for Unevaluated {
                fn dependencies(&self) -> $crate::expression::Dependency {
                    self.get_dependencies()
                }

                fn collect_global_state_refs(&self, refs: &mut $crate::collections::VecSet<String>) {
                    Unevaluated::collect_global_state_refs(self, refs);
                }
            }

            impl PossiblyEvaluated {
                /// The value of each property for the feature in `context`.
                pub fn evaluate_all(&self, context: &$crate::expression::EvaluationContext<'_>) -> Values {
                    Values {
                        $($field: self.$field.evaluate(context),)+
                    }
                }
            }

            $crate::__possibly_evaluated_methods!($($field: $kind),+);
        }
    };
}

/// Declares a layer type's property sets, as [`properties!`] does, and its
/// property table: `PROPERTIES`, with the paint properties and then the
/// layout properties in the order the sets list them, and `visit`, which
/// calls a visitor with the field of a slot of the table. A type's
/// [`StyleLayerType`](crate::layer::StyleLayerType) impl names both:
///
/// ```ignore
/// mln_style::layer_properties!(
///     /// The paint properties of an `org.example.dot` layer.
///     pub mod paint: paint {
///         dot_radius: data_driven f32 = 5.0,
///         dot_color: data_driven Color = Color::black(),
///     }
///     use mln_style::color::Color;
/// );
///
/// impl StyleLayerType for Dot {
///     const PROPERTIES: &'static PropertyTable = &PROPERTIES;
///     type PaintProperties = paint::Transitionable;
///     type LayoutProperties = NoProperties;
///     fn visit_property<V: PropertyVisitor<LayerStyle<Self>>>(slot: usize, visitor: V) -> Option<V::Output> {
///         visit(slot, visitor)
///     }
///     // ...
/// }
/// ```
///
/// The paint set comes first and the layout set second; a type may have
/// either alone. A property's style-spec name is its field's, with each `_`
/// a `-`, and a data-driven kind makes it data-driven in the table. The
/// sets can't have `derived` values.
#[macro_export]
macro_rules! layer_properties {
    ($(#[$pmeta:meta])* $pvis:vis mod $paint:ident: paint {
        $($pfield:ident: $pkind:ident $pt:ty = $pdefault:expr),+ $(,)?
    } $(#[$lmeta:meta])* $lvis:vis mod $layout:ident: layout {
        $($lfield:ident: $lkind:ident $lt:ty = $ldefault:expr),+ $(,)?
    } $(use $($import:path),+;)?) => {
        // Each set imports the paths both sets need.
        $crate::properties!(
            $(#[$pmeta])*
            #[allow(unused_imports, reason = "the imports are those of both sets")]
            $pvis mod $paint: paint { $($pfield: $pkind $pt = $pdefault,)+ } $(use $($import),+;)?
        );
        $crate::properties!(
            $(#[$lmeta])*
            #[allow(unused_imports, reason = "the imports are those of both sets")]
            $lvis mod $layout: layout { $($lfield: $lkind $lt = $ldefault,)+ } $(use $($import),+;)?
        );
        $crate::__property_table!(
            paint $paint { $($pfield $pkind)+ }
            layout $layout { $($lfield $lkind)+ }
        );
    };
    ($(#[$pmeta:meta])* $pvis:vis mod $paint:ident: paint {
        $($pfield:ident: $pkind:ident $pt:ty = $pdefault:expr),+ $(,)?
    } $(use $($import:path),+;)?) => {
        $crate::properties!(
            $(#[$pmeta])* $pvis mod $paint: paint { $($pfield: $pkind $pt = $pdefault,)+ } $(use $($import),+;)?
        );
        $crate::__property_table!(paint $paint { $($pfield $pkind)+ } layout);
    };
    ($(#[$lmeta:meta])* $lvis:vis mod $layout:ident: layout {
        $($lfield:ident: $lkind:ident $lt:ty = $ldefault:expr),+ $(,)?
    } $(use $($import:path),+;)?) => {
        $crate::properties!(
            $(#[$lmeta])* $lvis mod $layout: layout { $($lfield: $lkind $lt = $ldefault,)+ } $(use $($import),+;)?
        );
        $crate::__property_table!(paint layout $layout { $($lfield $lkind)+ });
    };
}

/// `layer_properties!`'s `PROPERTIES` and `visit`, for the paint set
/// `$paint` and the layout set `$layout`, either of which may be missing.
#[doc(hidden)]
#[macro_export]
macro_rules! __property_table {
    (
        paint $($paint:ident { $($pfield:ident $pkind:ident)+ })?
        layout $($layout:ident { $($lfield:ident $lkind:ident)+ })?
    ) => {
        /// The type's properties, by slot.
        pub(crate) static PROPERTIES: $crate::property::table::PropertyTable = {
            const COUNT: usize =
                <[&str]>::len(&[$($(::core::stringify!($pfield),)+)? $($(::core::stringify!($lfield),)+)?]);
            static INFO: [$crate::property::table::PropertyInfo; COUNT] = [
                $($($crate::__property_info!(paint $pfield $pkind),)+)?
                $($($crate::__property_info!(layout $lfield $lkind),)+)?
            ];
            static BY_NAME: [u8; COUNT] = $crate::__private::slots_by_name(&INFO);
            $crate::property::table::PropertyTable::new(&INFO, &BY_NAME)
        };

        /// Calls `visitor` with the field of the property in `slot` of
        /// `PROPERTIES`.
        pub(crate) fn visit<T, V>(slot: usize, visitor: V) -> ::core::option::Option<V::Output>
        where
            T: $crate::layer::StyleLayerType<
                $(PaintProperties = $paint::Transitionable,)?
                $(LayoutProperties = $layout::Unevaluated,)?
            >,
            V: $crate::property::table::PropertyVisitor<$crate::layer::LayerStyle<T>>,
        {
            let mut _next = 0;
            $($(
                if slot == _next {
                    return ::core::option::Option::Some(visitor.paint(|s| &s.paint.$pfield, |s| &mut s.paint.$pfield));
                }
                _next += 1;
            )+)?
            $($(
                if slot == _next {
                    return ::core::option::Option::Some(visitor.layout(|s| &s.layout.$lfield, |s| &mut s.layout.$lfield));
                }
                _next += 1;
            )+)?
            ::core::option::Option::None
        }
    };
}

/// The table entry of the property in the field `$field` of a `$scope`
/// set, of the kind `$kind`.
#[doc(hidden)]
#[macro_export]
macro_rules! __property_info {
    ($scope:ident $field:ident $kind:ident) => {{
        const FIELD: &str = ::core::stringify!($field);
        const NAME: [u8; FIELD.len()] = $crate::__private::property_name(FIELD);
        const INFO: $crate::property::table::PropertyInfo =
            // Replacing ASCII bytes keeps the field's UTF-8 valid.
            $crate::property::table::PropertyInfo::$scope(match ::core::str::from_utf8(&NAME) {
                ::core::result::Result::Ok(name) => name,
                ::core::result::Result::Err(_) => FIELD,
            });
        $crate::__property_info!(@flags INFO $kind)
    }};
    (@flags $info:ident data_driven) => {
        $info.data_driven()
    };
    (@flags $info:ident integer_zoom) => {
        $info.data_driven()
    };
    (@flags $info:ident cross_faded_data_driven) => {
        $info.data_driven().cross_faded()
    };
    (@flags $info:ident cross_faded_or_data_driven) => {
        $info.data_driven().cross_faded()
    };
    (@flags $info:ident $kind:ident) => {
        $info
    };
}

/// The methods of a set's `PossiblyEvaluated`.
#[doc(hidden)]
#[macro_export]
macro_rules! __possibly_evaluated_methods {
    ($($field:ident: $kind:ident),+) => {
        impl PossiblyEvaluated {
            /// The dependencies of the values that still differ by feature.
            pub fn get_dependencies(&self) -> $crate::expression::Dependency {
                let mut result = $crate::expression::Dependency::NONE;
                $(result |= self.$field.dependencies();)+
                result
            }

            pub fn collect_global_state_refs(&self, refs: &mut $crate::collections::VecSet<String>) {
                $(if let Some(property_refs) = self.$field.global_state_refs() {
                    refs.extend(property_refs.iter().cloned());
                })+
            }

            /// Bit i is set when the i-th data-driven property is the same
            /// for every feature.
            pub fn constants_mask(&self) -> u64 {
                let mut result = 0u64;
                let mut _index = 0u32;
                $(if $crate::__property_data_driven!($kind) {
                    if self.$field.is_constant() {
                        result |= 1u64 << _index;
                    }
                    _index += 1;
                })+
                result
            }
        }
    };
}

/// What an overridable property's override reads: whether a formatted
/// section sets a text color.
pub trait OverridableSection {
    fn has_text_color(&self) -> bool;
}

impl OverridableSection for crate::expression::formatted::FormattedSection {
    fn has_text_color(&self) -> bool {
        self.text_color.is_some()
    }
}

impl OverridableSection for crate::expression::forms::format::FormatExpressionSection {
    fn has_text_color(&self) -> bool {
        self.text_color.is_some()
    }
}
