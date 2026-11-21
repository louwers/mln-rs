use std::sync::Arc;

use crate::conversion::Convertible;
use crate::expression::expr::{Dependency, EvalResult, EvaluationContext, Expr};
use crate::expression::functions::{Eval, Functions, OverloadSpec, Params, RegisterError, Role};
use crate::expression::parsing_context::ParsingContext;
use crate::expression::type_::Type;
use crate::expression::value::Value;

fn twice(args: &[Expr], cx: &EvaluationContext<'_>) -> EvalResult<f64> {
    Ok(2.0 * args[0].eval_num(cx)?)
}

fn spec(params: Params, role: Role) -> OverloadSpec {
    OverloadSpec { result: Type::Number, params, dependencies: Dependency::NONE, role, eval: Eval::Number(twice) }
}

fn parse(functions: &Arc<Functions>, json: &str) -> Result<Expr, String> {
    let document: serde_json::Value = serde_json::from_str(json).unwrap();
    let mut ctx = ParsingContext::new(functions);
    ctx.parse_expression(&Convertible::from(&document), None).ok_or_else(|| ctx.get_combined_errors())
}

#[test]
fn a_registered_function_parses_and_evaluates() {
    let mut functions = Functions::new();
    functions.add("twice", vec![spec(Params::Fixed(vec![Type::Number]), Role::Plain)]).unwrap();
    let functions = Arc::new(functions);

    let expression = parse(&functions, r#"["twice", ["get", "x"]]"#);
    assert!(expression.is_err(), "\"get\" isn't registered");

    let expression = parse(&functions, r#"["twice", ["number", 3]]"#).unwrap();
    assert_eq!(expression.literal_value(), Some(&Value::Number(6.0)), "constant calls fold");
}

#[test]
fn names_resolve_against_the_context_s_functions() {
    let functions = Arc::new(Functions::new());
    let error = parse(&functions, r#"["zoom"]"#).unwrap_err();
    assert!(error.contains(r#"Unknown expression "zoom""#), "{error}");
    assert!(parse(&Functions::with_builtins(), r#"["+", 1, ["zoom"]]"#).is_ok());
}

#[test]
fn add_refuses_what_parsing_couldn_t_resolve() {
    let fixed = || spec(Params::Fixed(vec![Type::Number]), Role::Plain);
    let mut functions = Functions::new();
    functions.add("twice", vec![fixed()]).unwrap();
    assert_eq!(functions.add("twice", vec![fixed()]), Err(RegisterError::Duplicate("twice".into())));
    assert_eq!(functions.add("let", vec![fixed()]), Err(RegisterError::SpecialForm("let".into())));
    assert_eq!(functions.add("none", vec![]), Err(RegisterError::OverloadCount("none".into())));
    let varargs = spec(Params::Varargs(Type::Number), Role::Plain);
    assert_eq!(functions.add("two", vec![fixed(), varargs]), Err(RegisterError::AmbiguousArity("two".into())));
    let mismatch = OverloadSpec { result: Type::String, ..fixed() };
    assert_eq!(functions.add("mismatch", vec![mismatch]), Err(RegisterError::EvalMismatch("mismatch".into())));
    functions.add("level", vec![spec(Params::Fixed(vec![]), Role::Zoom)]).unwrap();
    let zoom = spec(Params::Fixed(vec![]), Role::Zoom);
    assert_eq!(functions.add("zoom", vec![zoom]), Err(RegisterError::RoleTaken("zoom".into())));
}

#[test]
fn the_builtins_register_once() {
    let mut functions = Functions::new();
    assert_eq!(crate::expression::functions::register_builtin_functions(&mut functions), Ok(()));
    assert!(matches!(
        crate::expression::functions::register_builtin_functions(&mut functions),
        Err(RegisterError::Duplicate(_))
    ));
    let overloads = functions.get("-").unwrap().overloads();
    assert_eq!(overloads.len(), 2, "binary minus, then negation");
}

#[test]
fn a_compound_expression_crosses_by_name() {
    let functions = Functions::with_builtins();
    let expression = parse(&functions, r#"["-", ["get", "x"]]"#).unwrap();
    let encoded = mln_wire::to_vec(&expression).unwrap();

    let decoded: Expr = mln_wire::from_slice_in(&encoded, &*functions).unwrap();
    assert_eq!(decoded, expression);
    assert_eq!(decoded.dependencies(), expression.dependencies());

    assert!(mln_wire::from_slice::<Expr>(&encoded).is_err(), "no functions to resolve against");
    assert!(mln_wire::from_slice_in::<Expr>(&encoded, &Functions::new()).is_err());
}
