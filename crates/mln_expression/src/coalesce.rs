use mln_conversion::Convertible;

use crate::check_subtype::check_subtype;
use crate::expression::{
    EvaluationContext, EvaluationResult, Expression, ExpressionBase, Kind, children_equal, collect_dependencies,
    downcast,
};
use crate::image::Image;
use crate::parsing_context::{ParseResult, ParsingContext, TypeAnnotationOption};
use crate::type_::Type;
use crate::value::{Value, from_expression_value};

pub(crate) type Args = Vec<Box<dyn Expression>>;

pub(crate) struct Coalesce {
    base: ExpressionBase,
    args: Args,
}

impl Coalesce {
    #[must_use]
    pub(crate) fn new(type_: Type, args: Args) -> Self {
        Coalesce { base: ExpressionBase::new(Kind::Coalesce, type_, collect_dependencies(&args)), args }
    }

    #[must_use]
    pub(crate) fn get_length(&self) -> usize {
        self.args.len()
    }

    #[must_use]
    pub(crate) fn get_child(&self, i: usize) -> &dyn Expression {
        &*self.args[i]
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

        let mut args = Args::with_capacity(length - 1);
        for i in 1..length {
            let parsed = ctx.parse(&value.array_member(i), i, output_type.clone(), Some(TypeAnnotationOption::Omit))?;
            if output_type.is_none() {
                output_type = Some(parsed.get_type().clone());
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
            .is_some_and(|expected| args.iter().any(|arg| check_subtype(expected, arg.get_type()).is_some()));

        Some(Box::new(Coalesce::new(if needs_annotation { Type::Value } else { output_type }, args)))
    }
}

impl Expression for Coalesce {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let mut result: EvaluationResult = Ok(Value::Null);
        let mut args_count = self.args.len();
        let mut requested_image: Option<Image> = None;
        for arg in &self.args {
            args_count -= 1;
            result = arg.evaluate(params);
            // We need to keep track of the first requested image in a
            // coalesce statement. If coalesce can't find a valid image, we
            // return the first requested image.
            if *self.get_type() == Type::Image
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

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        for arg in &self.args {
            visit(&**arg);
        }
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::Coalesce {
            let rhs = downcast::<Coalesce>(e).expect("Coalesce");
            return children_equal(&self.args, &rhs.args);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        let mut result = Vec::new();
        for arg in &self.args {
            result.extend(arg.possible_outputs());
        }
        result
    }

    fn get_operator(&self) -> String {
        "coalesce".to_owned()
    }
}
