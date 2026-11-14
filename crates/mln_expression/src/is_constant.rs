use crate::compound_expression::Op;
use crate::expression::{Dependency, Expr, Node};
use crate::value::Value;

pub(crate) fn is_global_property_constant(expression: &Expr, properties: &[&str]) -> bool {
    if let Some(e) = expression.as_compound()
        && properties.contains(&e.signature().name)
    {
        return false;
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

#[must_use]
pub fn is_feature_constant(expression: &Expr) -> bool {
    if let Some(e) = expression.as_compound() {
        let name = e.signature().name;
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

    if matches!(expression.node(), Node::FormatSectionOverride(_) | Node::Within(_) | Node::Distance(_)) {
        return false;
    }

    if matches!(expression.node(), Node::Collator(_)) {
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

#[must_use]
pub fn is_zoom_constant(e: &Expr) -> bool {
    is_global_property_constant(e, &ZOOM_PROPERTY)
}

/// Returns true if expression does not depend on information provided by the
/// runtime.
#[must_use]
pub fn is_runtime_constant(expression: &Expr) -> bool {
    if matches!(expression.node(), Node::Image(_)) {
        return false;
    }

    if let Some(e) = expression.as_compound()
        && e.signature().op == Op::GlobalState
    {
        // The result depends on the style's global state, which can be
        // changed at runtime, so the expression is not runtime constant.
        return false;
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
#[must_use]
pub fn feature_property_keys(expression: &Expr) -> Option<Vec<String>> {
    let mut keys = Vec::new();
    if !collect_feature_property_keys(expression, &mut keys) {
        return None;
    }
    keys.sort_unstable();
    keys.dedup();
    Some(keys)
}

fn collect_feature_property_keys(expression: &Expr, keys: &mut Vec<String>) -> bool {
    match expression.node() {
        Node::Within(_) | Node::Distance(_) | Node::FormatSectionOverride(_) => return false,
        Node::Compound(e) => {
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
                    match e.args().first().and_then(Expr::literal_value) {
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
