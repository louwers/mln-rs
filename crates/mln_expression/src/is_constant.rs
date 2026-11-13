use crate::compound_expression::CompoundExpression;
use crate::expression::{Dependency, Expression, Kind, downcast};
use crate::literal::Literal;
use crate::value::Value;

pub(crate) fn is_global_property_constant(expression: &dyn Expression, properties: &[&str]) -> bool {
    if expression.get_kind() == Kind::CompoundExpression {
        let e = downcast::<CompoundExpression>(expression).expect("CompoundExpression");
        for property in properties {
            if e.get_operator() == *property {
                return false;
            }
        }
    }

    let mut is_constant = true;
    expression.each_child(&mut |e| {
        if is_constant && !is_global_property_constant(e, properties) {
            is_constant = false;
        }
    });
    is_constant
}

const FILTER: &str = "filter-";

pub fn is_feature_constant(expression: &dyn Expression) -> bool {
    if expression.get_kind() == Kind::CompoundExpression {
        let e = downcast::<CompoundExpression>(expression).expect("CompoundExpression");
        let name = e.get_operator();
        let parameter_count = e.get_parameter_count();
        // Legacy filters begin with "filter-" and are never constant.
        if ((name == "get" || name == "has") && parameter_count == Some(1))
            || name == "properties"
            || name == "geometry-type"
            || name == "id"
            || name == "feature-state"
            || name.starts_with(FILTER)
        {
            return false;
        }
    }

    if expression.get_kind() == Kind::FormatSectionOverride {
        return false;
    }

    if expression.get_kind() == Kind::Within {
        return false;
    }

    if expression.get_kind() == Kind::Distance {
        return false;
    }

    if expression.get_kind() == Kind::CollatorExpression {
        // Although the results of a Collator expression with fixed arguments
        // generally shouldn't change between executions, we can't serialize
        // them as constant expressions because results change based on
        // environment.
        return false;
    }

    let mut feature_constant = true;
    expression.each_child(&mut |e| {
        if feature_constant && !is_feature_constant(e) {
            feature_constant = false;
        }
    });
    feature_constant
}

const ZOOM_PROPERTY: [&str; 1] = ["zoom"];

pub fn is_zoom_constant(e: &dyn Expression) -> bool {
    is_global_property_constant(e, &ZOOM_PROPERTY)
}

/// Returns true if expression does not depend on information provided by the
/// runtime.
pub fn is_runtime_constant(expression: &dyn Expression) -> bool {
    if expression.get_kind() == Kind::ImageExpression {
        return false;
    }

    if expression.get_kind() == Kind::CompoundExpression {
        let e = downcast::<CompoundExpression>(expression).expect("CompoundExpression");
        if e.get_operator() == "global-state" {
            // The result depends on the style's global state, which can be
            // changed at runtime, so the expression is not runtime constant.
            return false;
        }
    }

    let mut runtime_constant = true;
    expression.each_child(&mut |e| {
        if runtime_constant && !is_runtime_constant(e) {
            runtime_constant = false;
        }
    });
    runtime_constant
}

/// Port-only: the property keys `expression` reads, sorted and without
/// duplicates, if they and the geometry type are all it reads of a feature:
/// not its id, its geometry, its feature state, all its properties or a key
/// it computes. The expression then gives features with the same geometry
/// type and the same values under those keys the same result.
pub fn feature_property_keys(expression: &dyn Expression) -> Option<Vec<String>> {
    let mut keys = Vec::new();
    if !collect_feature_property_keys(expression, &mut keys) {
        return None;
    }
    keys.sort_unstable();
    keys.dedup();
    Some(keys)
}

fn collect_feature_property_keys(expression: &dyn Expression, keys: &mut Vec<String>) -> bool {
    match expression.get_kind() {
        Kind::Within | Kind::Distance | Kind::FormatSectionOverride => return false,
        Kind::CompoundExpression => {
            let Some(e) = downcast::<CompoundExpression>(expression) else {
                return false;
            };
            let signature = e.signature();
            if signature.dependencies.intersects(Dependency::FEATURE) {
                let reads_key = match signature.name {
                    "get" | "has" => e.get_parameter_count() == Some(1),
                    "filter-==" | "filter-<" | "filter->" | "filter-<=" | "filter->=" | "filter-has" | "filter-in" => {
                        true
                    }
                    "geometry-type" | "filter-type-==" | "filter-type-in" => false,
                    _ => return false,
                };
                if reads_key {
                    let key = e.args().first().and_then(|arg| downcast::<Literal>(&**arg));
                    match key.map(Literal::get_value) {
                        Some(Value::String(key)) => keys.push(key.clone()),
                        _ => return false,
                    }
                }
            }
        }
        _ => {}
    }
    let mut groupable = true;
    expression.each_child(&mut |e| {
        groupable = groupable && collect_feature_property_keys(e, keys);
    });
    groupable
}
