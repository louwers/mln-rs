//! What an expression reads: its inputs ([`Dependency`]), the global state
//! it names, and the feature properties it reads.

use std::fmt;

use crate::collections::VecSet;
use crate::expression::functions::Role;
use crate::expression::value::Value;
use crate::expression::{Expr, Node};

/// The inputs an expression depends on, as a bit set.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Dependency(pub u32);

impl Dependency {
    pub const NONE: Dependency = Dependency(0);
    /// Data reference
    pub const FEATURE: Dependency = Dependency(1 << 0);
    /// Image reference (equivalent to not "runtime constant")
    pub const IMAGE: Dependency = Dependency(1 << 1);
    /// Zoom level
    pub const ZOOM: Dependency = Dependency(1 << 2);
    /// Create variable binding ("let")
    pub const BIND: Dependency = Dependency(1 << 4);
    /// Use variable binding
    pub const VAR: Dependency = Dependency(1 << 5);
    /// Property override
    pub const OVERRIDE: Dependency = Dependency(1 << 6);
    /// Elevation from DEM
    pub const ELEVATION: Dependency = Dependency(1 << 7);
    /// Global state ("global-state")
    pub const GLOBAL_STATE: Dependency = Dependency(1 << 8);
    /// Feature state ("feature-state"). Always set along with `FEATURE`.
    pub const FEATURE_STATE: Dependency = Dependency(1 << 9);
    /// Inputs that only some properties provide while they evaluate:
    /// "heatmap-density", "line-progress" and "accumulated".
    pub const PROPERTY_INPUT: Dependency = Dependency(1 << 10);

    /// Whether the two sets share an input.
    #[must_use]
    pub fn intersects(self, other: Dependency) -> bool {
        (self.0 & other.0) != 0
    }
}

impl std::ops::BitOr for Dependency {
    type Output = Dependency;
    fn bitor(self, rhs: Dependency) -> Dependency {
        Dependency(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Dependency {
    fn bitor_assign(&mut self, rhs: Dependency) {
        self.0 |= rhs.0;
    }
}

impl fmt::Display for Dependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Dependency::NONE {
            return f.write_str("None");
        }
        let mut any = false;
        for (dep, name) in [
            (Dependency::FEATURE, "Feature"),
            (Dependency::IMAGE, "Image"),
            (Dependency::ZOOM, "Zoom"),
            (Dependency::BIND, "Bind"),
            (Dependency::VAR, "Var"),
            (Dependency::OVERRIDE, "Override"),
            (Dependency::ELEVATION, "Elevation"),
            (Dependency::GLOBAL_STATE, "GlobalState"),
            (Dependency::FEATURE_STATE, "FeatureState"),
            (Dependency::PROPERTY_INPUT, "PropertyInput"),
        ] {
            if self.intersects(dep) {
                if any {
                    f.write_str("|")?;
                }
                f.write_str(name)?;
                any = true;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for Dependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Collect the names of the global-state properties referenced by
/// "global-state" expressions in the given expression tree.
pub fn collect_global_state_refs(expression: &Expr, refs: &mut VecSet<String>) {
    if !expression.has(Dependency::GLOBAL_STATE) {
        return;
    }
    if let Some(compound) = expression.as_compound()
        && compound.overload().role == Role::GlobalState
    {
        // The property name argument is enforced to be a string literal
        // at parse time.
        if let Some(Value::String(s)) = compound.args().first().and_then(Expr::literal_value) {
            refs.insert(s.clone());
        }
        return;
    }
    expression.each_child(&mut |child| collect_global_state_refs(child, refs));
}

/// Whether any of the global-state keys referenced by an expression is among
/// the changed keys. A null set on either side means "unknown" and is
/// conservatively treated as a match.
#[must_use]
pub fn global_state_refs_intersect(refs: Option<&VecSet<String>>, changed_keys: Option<&VecSet<String>>) -> bool {
    match (refs, changed_keys) {
        (Some(refs), Some(changed_keys)) => refs.iter().any(|key| changed_keys.contains(key)),
        _ => true,
    }
}

/// The property keys `expression` reads, sorted and without
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
            let overload = e.overload();
            if overload.dependencies.intersects(Dependency::FEATURE) {
                let reads_key = match overload.role {
                    Role::FeatureProperty => true,
                    Role::GeometryType => false,
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
