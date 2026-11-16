//! The expression functions: the operators that the parser resolves by name
//! against a [`Functions`] value and that evaluate their already-parsed
//! arguments.
//!
//! A function has one or more overloads, each with its parameter and result
//! types, the inputs it reads ([`Dependency`]) and an evaluator. Parsing tries
//! a name's overloads in the order they were added. The expression syntax
//! (`let`, `match`, `case`, `interpolate` and the other forms that parse their
//! own arguments) isn't a function and can't be registered as one.
//!
//! A parsed compound expression keeps the overload it matched. On the worker
//! codec it crosses as its name and the overload's position, which the
//! reading side resolves against the [`Functions`] that [`decode_with`]
//! names.

pub(crate) mod builtin;

use std::borrow::Cow;
use std::cell::Cell;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock};

use mln_util::Color;
use mln_util::hash::HashMap;

pub use self::builtin::register_builtin_functions;
use crate::expression::{Dependency, EvalResult, EvaluationContext, EvaluationResult, Expr};
use crate::parsing_context::is_special_form;
use crate::type_::Type;

/// The parameter list of an overload: fixed types, or any number of
/// arguments of one type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Params {
    Fixed(Vec<Type>),
    Varargs(Type),
}

impl Params {
    /// Whether some number of arguments fits both.
    fn overlaps(&self, other: &Params) -> bool {
        match (self, other) {
            (Params::Fixed(a), Params::Fixed(b)) => a.len() == b.len(),
            _ => true,
        }
    }
}

/// What the built-in static analyses may assume about a function. `Plain`
/// for all but six built-ins. Only one function may claim `Zoom` and only
/// one `GlobalState` ([`Functions::add`] rejects a second).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Plain,
    /// Reads the feature property whose key is the literal first argument
    /// (`get`, `has`).
    FeatureProperty,
    /// Reads the feature's id (`id`).
    FeatureId,
    /// Reads the feature's geometry type (`geometry-type`).
    GeometryType,
    /// The zoom level that a top-level `step` or `interpolate` may take as
    /// its input (`zoom`).
    Zoom,
    /// Reads the global-state property whose key is the literal first
    /// argument (`global-state`).
    GlobalState,
}

/// An overload's evaluator, typed by its result so that a number, boolean,
/// string or color evaluates without a [`Value`](crate::Value) in between.
/// `Value` is for every other result type.
#[derive(Clone, Copy, Debug)]
pub enum Eval {
    Number(fn(&[Expr], &EvaluationContext<'_>) -> EvalResult<f64>),
    Boolean(fn(&[Expr], &EvaluationContext<'_>) -> EvalResult<bool>),
    String(for<'e> fn(&'e [Expr], &EvaluationContext<'_>) -> EvalResult<Cow<'e, str>>),
    Color(fn(&[Expr], &EvaluationContext<'_>) -> EvalResult<Color>),
    Value(fn(&[Expr], &EvaluationContext<'_>) -> EvaluationResult),
}

impl Eval {
    /// Whether this evaluator gives values of `result`.
    fn matches(&self, result: &Type) -> bool {
        match self {
            Eval::Number(_) => *result == Type::Number,
            Eval::Boolean(_) => *result == Type::Boolean,
            Eval::String(_) => *result == Type::String,
            Eval::Color(_) => *result == Type::Color,
            Eval::Value(_) => !matches!(result, Type::Number | Type::Boolean | Type::String | Type::Color),
        }
    }
}

/// An overload as [`Functions::add`] takes it.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct OverloadSpec {
    pub result: Type,
    pub params: Params,
    pub dependencies: Dependency,
    pub role: Role,
    pub eval: Eval,
}

/// One overload of a function.
#[derive(Debug)]
pub struct Overload {
    pub(crate) name: Arc<str>,
    /// The overload's position among its name's.
    pub(crate) index: u8,
    pub result: Type,
    pub params: Params,
    pub dependencies: Dependency,
    pub role: Role,
    pub eval: Eval,
}

impl Overload {
    /// The function's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// A function: its name and its overloads, in the order parsing tries them.
#[derive(Debug)]
pub struct FunctionDef {
    name: Arc<str>,
    overloads: Box<[Arc<Overload>]>,
}

impl FunctionDef {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn overloads(&self) -> &[Arc<Overload>] {
        &self.overloads
    }
}

/// Why [`Functions::add`] refused a function.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    #[error("\"{0}\" is already registered")]
    Duplicate(String),
    #[error("\"{0}\" is an expression form, not a function")]
    SpecialForm(String),
    #[error("\"{0}\" needs between 1 and 256 overloads")]
    OverloadCount(String),
    #[error("two overloads of \"{0}\" take the same number of arguments")]
    AmbiguousArity(String),
    #[error("an overload of \"{0}\" evaluates to another type than its result")]
    EvalMismatch(String),
    #[error("\"{0}\" claims a role that another function has")]
    RoleTaken(String),
}

/// The expression functions that parsing resolves names against.
#[derive(Debug, Default)]
pub struct Functions {
    defs: Vec<FunctionDef>,
    by_name: HashMap<Arc<str>, usize>,
}

impl Functions {
    /// No functions.
    #[must_use]
    pub fn new() -> Self {
        Functions::default()
    }

    /// The style specification's functions. Every call returns the same
    /// value, built on the first: it never changes, so sharing it is a cache.
    #[must_use]
    pub fn with_builtins() -> Arc<Functions> {
        static BUILTINS: LazyLock<Arc<Functions>> = LazyLock::new(|| {
            let mut functions = Functions::new();
            // Into no functions, the built-ins register: no two share a name
            // or a role, and none is an expression form.
            let registered = register_builtin_functions(&mut functions);
            debug_assert_eq!(registered, Ok(()));
            Arc::new(functions)
        });
        Arc::clone(&BUILTINS)
    }

    /// Adds the function `name` with its overloads, in the order parsing
    /// tries them. Fails for a name that is taken or is an expression form,
    /// two overloads that take the same number of arguments, an evaluator of
    /// another type than its overload's result, or a role that only one
    /// function may have and another has.
    #[doc(hidden)]
    pub fn add(&mut self, name: &str, overloads: Vec<OverloadSpec>) -> Result<(), RegisterError> {
        if is_special_form(name) {
            return Err(RegisterError::SpecialForm(name.to_owned()));
        }
        if self.by_name.contains_key(name) {
            return Err(RegisterError::Duplicate(name.to_owned()));
        }
        if overloads.is_empty() || overloads.len() > usize::from(u8::MAX) + 1 {
            return Err(RegisterError::OverloadCount(name.to_owned()));
        }
        // Parsing parses the arguments for the one overload that takes as many
        // as it finds, so two of one arity would leave the second unreachable.
        for (i, a) in overloads.iter().enumerate() {
            if overloads[..i].iter().any(|b| a.params.overlaps(&b.params)) {
                return Err(RegisterError::AmbiguousArity(name.to_owned()));
            }
        }
        if overloads.iter().any(|overload| !overload.eval.matches(&overload.result)) {
            return Err(RegisterError::EvalMismatch(name.to_owned()));
        }
        for role in [Role::Zoom, Role::GlobalState] {
            let claims = |def: &FunctionDef| def.overloads.iter().any(|overload| overload.role == role);
            if overloads.iter().any(|overload| overload.role == role) && self.defs.iter().any(claims) {
                return Err(RegisterError::RoleTaken(name.to_owned()));
            }
        }
        let name: Arc<str> = Arc::from(name);
        let overloads = (0u8..=u8::MAX)
            .zip(overloads)
            .map(|(index, spec)| {
                Arc::new(Overload {
                    name: Arc::clone(&name),
                    index,
                    result: spec.result,
                    params: spec.params,
                    dependencies: spec.dependencies,
                    role: spec.role,
                    eval: spec.eval,
                })
            })
            .collect();
        self.by_name.insert(Arc::clone(&name), self.defs.len());
        self.defs.push(FunctionDef { name, overloads });
        Ok(())
    }

    /// The function named `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&FunctionDef> {
        self.by_name.get(name).map(|&index| &self.defs[index])
    }

    pub(crate) fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Feeds `hasher` what parsing and decoding resolve against: each
    /// function's name and its overloads' parameter and result types,
    /// dependencies and roles, in the order they were added. Two values
    /// filled by the same calls hash alike with a deterministic hasher.
    pub fn digest(&self, hasher: &mut impl Hasher) {
        self.defs.len().hash(hasher);
        for def in &self.defs {
            def.name.hash(hasher);
            def.overloads.len().hash(hasher);
            for overload in &def.overloads {
                let Overload { name: _, index: _, result, params, dependencies, role, eval: _ } = &**overload;
                (result, params, dependencies, role).hash(hasher);
            }
        }
    }
}

thread_local! {
    /// The functions that compound expressions read on this thread resolve
    /// against, while a `decode_with` runs.
    static DECODING: Cell<Option<Arc<Functions>>> = const { Cell::new(None) };
}

/// Runs `f`, in which reading an [`Expr`] from the worker codec resolves
/// compound expressions against `functions`. Calls nest: the innermost
/// one's functions apply, and the outer one's again after it returns.
pub fn decode_with<R>(functions: &Arc<Functions>, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Arc<Functions>>);

    impl Drop for Restore {
        fn drop(&mut self) {
            DECODING.set(self.0.take());
        }
    }

    let _restore = Restore(DECODING.replace(Some(Arc::clone(functions))));
    f()
}

/// The overload `index` of `name` among the functions of the enclosing
/// [`decode_with`].
pub(crate) fn decoding_overload(name: &str, index: u8) -> Result<Arc<Overload>, &'static str> {
    let functions = DECODING.take();
    let overload = match &functions {
        None => Err("an expression function outside of a decoding session"),
        Some(functions) => match functions.get(name) {
            None => Err("unknown expression function"),
            Some(def) => def.overloads.get(usize::from(index)).cloned().ok_or("no such overload of a function"),
        },
    };
    DECODING.set(functions);
    overload
}
