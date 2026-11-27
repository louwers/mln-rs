use std::cell::Cell;

use crate::collections::VecSet;
use crate::conversion::Convertible;
use crate::expression::image::Image;
use crate::expression::parse::{ParseResult, ParsingContext};
use crate::expression::type_::Type;
use crate::expression::value::{Value, to_string};
use crate::expression::{EvaluationContext, EvaluationResult, Expr, Node};
use crate::feature::Value as MbglValue;

/// Collects the images that `image` expressions find unavailable. An
/// evaluation whose [`EvaluationContext`] carries both the available images
/// and a collector records each non-empty id it doesn't find. A tile layout
/// asks for these images too, so that it is redone when one is added, even
/// where a `coalesce` fell back past it.
#[derive(Default)]
pub struct UnavailableImages {
    // A `Cell`, as evaluation reaches the collector through a shared
    // context; `record` takes the set out and puts it back.
    ids: Cell<VecSet<String>>,
}

impl UnavailableImages {
    #[must_use]
    pub fn new() -> Self {
        UnavailableImages::default()
    }

    fn record(&self, id: &str) {
        let mut ids = self.ids.take();
        if !ids.contains(id) {
            ids.insert(id.to_owned());
        }
        self.ids.set(ids);
    }

    /// The images recorded.
    #[must_use]
    pub fn into_inner(self) -> VecSet<String> {
        self.ids.into_inner()
    }
}

/// `["image", id]`: the image of an id, and whether it is available.
#[derive(Clone, PartialEq, mln_wire::Wire)]
pub struct ImageExpression {
    image_id: Expr,
}

impl ImageExpression {
    #[must_use]
    pub fn new_expr(image_id: Expr) -> Expr {
        Expr::new(Node::Image(Box::new(ImageExpression { image_id })))
    }

    pub(crate) fn image_id(&self) -> &Expr {
        &self.image_id
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        if value.array_length() != 2 {
            ctx.error("Expected two arguments.".to_owned());
            return None;
        }

        // Parse expression that evaluates to image id.
        let image_id_arg = value.array_member(1);
        let image_id_expr = ctx.parse(&image_id_arg, 1, Some(Type::String), None)?;

        Some(ImageExpression::new_expr(image_id_expr))
    }

    pub(crate) fn evaluate(&self, ctx: &EvaluationContext<'_>) -> EvaluationResult {
        let image_id_result = self.image_id.evaluate(ctx)?;

        // `toString(Value)` always produces a string.
        let evaluated_image_id = to_string(&image_id_result);

        let Some(available_images) = ctx.available_images else {
            return Ok(Value::Image(Image::with_available(evaluated_image_id, false)));
        };
        let available = available_images.contains(&evaluated_image_id);
        if !available
            && !evaluated_image_id.is_empty()
            && let Some(unavailable_images) = ctx.unavailable_images
        {
            unavailable_images.record(&evaluated_image_id);
        }
        Ok(Value::Image(Image::with_available(evaluated_image_id, available)))
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        MbglValue::Array(vec![MbglValue::String("image".to_owned()), self.image_id.serialize()])
    }
}
