// Ports include/mln/style/expression/image_expression.hpp and
// src/mln/style/expression/image_expression.cpp.

use std::sync::Arc;

use feature::Value as MbglValue;
use mln_conversion::Convertible;

use crate::expression::{Dependency, EvaluationContext, EvaluationResult, Expression, ExpressionBase, Kind, downcast};
use crate::image::Image;
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::{Value, to_string};

pub struct ImageExpression {
    base: ExpressionBase,
    image_id: Arc<dyn Expression>,
}

impl ImageExpression {
    pub fn new(image_id: Box<dyn Expression>) -> Self {
        ImageExpression {
            base: ExpressionBase::new(Kind::ImageExpression, Type::Image, image_id.dependencies() | Dependency::IMAGE),
            image_id: Arc::from(image_id),
        }
    }

    pub fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        let args_length = value.array_length();
        if args_length < 2 {
            ctx.error("Invalid number of arguments for 'image' expression.".to_owned());
            return None;
        }

        // Parse expression that evaluates to image id.
        let image_id_arg = value.array_member(1);
        let image_id_expr = ctx.parse(&image_id_arg, 1, Some(Type::String), None)?;

        Some(Box::new(ImageExpression::new(image_id_expr)))
    }
}

impl Expression for ImageExpression {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, ctx: &EvaluationContext<'_>) -> EvaluationResult {
        let image_id_result = self.image_id.evaluate(ctx)?;

        // `toString(Value)` always produces a string.
        let evaluated_image_id = to_string(&image_id_result);

        let available = ctx.available_images.is_some_and(|images| images.contains(&evaluated_image_id));
        Ok(Value::Image(Image::with_available(evaluated_image_id, available)))
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        visit(&*self.image_id);
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::ImageExpression {
            let rhs = downcast::<ImageExpression>(e).expect("ImageExpression");
            return self.image_id.equals(&*rhs.image_id);
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        vec![None]
    }

    fn serialize(&self) -> MbglValue {
        // Port-only: the legacy property functions wrap a token string's
        // image expression in a second one (a C++ quirk the conversion
        // keeps). `["image", ["image", ...]]` doesn't parse back, since
        // "image" takes a string; the outer one evaluates to the inner one's
        // image, so it serializes as the inner one.
        if *self.image_id.get_type() == Type::Image {
            return self.image_id.serialize();
        }
        MbglValue::Array(vec![MbglValue::String(self.get_operator()), self.image_id.serialize()])
    }

    fn get_operator(&self) -> String {
        "image".to_owned()
    }
}
