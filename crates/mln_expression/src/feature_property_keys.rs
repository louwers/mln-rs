use crate::compound_expression::Op;
use crate::expression::{Dependency, Expr, Node};
use crate::value::Value;

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
                let reads_key = match signature.op {
                    Op::Get
                    | Op::Has
                    | Op::FilterEq
                    | Op::FilterLt
                    | Op::FilterGt
                    | Op::FilterLe
                    | Op::FilterGe
                    | Op::FilterHas
                    | Op::FilterIn => true,
                    Op::GeometryType | Op::FilterTypeEq | Op::FilterTypeIn => false,
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
