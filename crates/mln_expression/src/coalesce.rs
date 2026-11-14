use mln_conversion::Convertible;

use crate::check_subtype::check_subtype;
use crate::expression::{EvalResult, EvaluationContext, EvaluationResult, Expr, Node, Output};
use crate::image::Image;
use crate::parsing_context::{ParseResult, ParsingContext, TypeAnnotationOption};
use crate::type_::Type;
use crate::value::{Value, from_expression_value};

/// `["coalesce", ...]`: the first operand that is not null.
#[derive(Clone, PartialEq)]
pub(crate) struct Coalesce {
    type_: Type,
    args: Box<[Expr]>,
}

impl Coalesce {
    pub(crate) fn new_expr(type_: Type, args: Vec<Expr>) -> Expr {
        Expr::new(Node::Coalesce(Box::new(Coalesce { type_, args: args.into() })))
    }

    pub(crate) fn get_type(&self) -> &Type {
        &self.type_
    }

    pub(crate) fn args(&self) -> &[Expr] {
        &self.args
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        debug_assert!(value.is_array());
        let length = value.array_length();
        if length < 2 {
            ctx.error("Expected at least one argument.".to_owned());
            return None;
        }

        let mut output_type: Option<Type> = None;
        let expected_type = ctx.get_expected();
        if let Some(expected) = &expected_type
            && *expected != Type::Value
        {
            output_type = Some(expected.clone());
        }

        let mut args = Vec::with_capacity(length - 1);
        for i in 1..length {
            let parsed = ctx.parse(&value.array_member(i), i, output_type.clone(), Some(TypeAnnotationOption::Omit))?;
            if output_type.is_none() {
                output_type = Some(parsed.get_type());
            }
            args.push(parsed);
        }
        let output_type = output_type.expect("coalesce output type");

        // Above, we parse arguments without inferred type annotation so that
        // they don't produce a runtime error for `null` input, which would
        // preempt the desired null-coalescing behavior. Thus, if any of our
        // arguments would have needed an annotation, we need to wrap the
        // enclosing coalesce expression with it instead.
        let needs_annotation = expected_type
            .as_ref()
            .is_some_and(|expected| args.iter().any(|arg| check_subtype(expected, &arg.get_type()).is_some()));

        Some(Coalesce::new_expr(if needs_annotation { Type::Value } else { output_type }, args))
    }

    pub(crate) fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let mut result: EvaluationResult = Ok(Value::Null);
        let mut args_count = self.args.len();
        let mut requested_image: Option<Image> = None;
        for arg in &self.args {
            args_count -= 1;
            result = arg.evaluate(params);
            // We need to keep track of the first requested image in a
            // coalesce statement. If coalesce can't find a valid image, we
            // return the first requested image.
            if self.type_ == Type::Image
                && let Ok(r) = &result
                && let Some(image) = from_expression_value::<Image>(r)
                && !image.is_available()
            {
                if requested_image.is_none() {
                    requested_image = Some(Image::new(image.id()));
                }
                if args_count == 0 {
                    result = Ok(Value::Image(requested_image.clone().unwrap()));
                }
                continue;
            }

            match &result {
                Err(_) => break,
                Ok(v) if *v != Value::Null => break,
                Ok(_) => {}
            }
        }
        result
    }

    /// The typed evaluation, for a coalesce of type `type_`. An operand of
    /// that type is never null, so the first one decides; an operand that
    /// the parser left a value (it omits annotations here) is skipped while
    /// it is null.
    pub(crate) fn eval_typed<'e, T: Output<'e>>(
        &'e self,
        params: &EvaluationContext<'_>,
        type_: &Type,
    ) -> EvalResult<T> {
        debug_assert_eq!(&self.type_, type_);
        for arg in &self.args {
            if arg.get_type() == *type_ {
                return T::eval(arg, params);
            }
            match arg.evaluate(params)? {
                Value::Null => {}
                value => return T::from_value(value),
            }
        }
        T::from_value(Value::Null)
    }
}
