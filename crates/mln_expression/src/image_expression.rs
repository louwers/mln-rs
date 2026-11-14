use std::cell::RefCell;

use feature::Value as MbglValue;
use mln_conversion::Convertible;
use mln_vec_map::VecSet;

use crate::expression::{EvaluationContext, EvaluationResult, Expr, Node};
use crate::image::Image;
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::{Value, to_string};

thread_local! {
    /// The images that `image` expressions found unavailable while an
    /// [`UnavailableImages`] records on this thread.
    static UNAVAILABLE: RefCell<Option<VecSet<String>>> = const { RefCell::new(None) };
}

/// Records the images that `image` expressions evaluated on this thread
/// against the available images find unavailable, from [`start`] until
/// [`finish`] or a drop. A tile layout asks for these images too, so that
/// it is redone when one is added, even where a `coalesce` fell back past
/// it. The record is per thread because evaluation is synchronous and the
/// style's evaluation calls don't carry a place to put it. A recorder
/// started while another records hides its images from the outer one.
///
/// [`start`]: UnavailableImages::start
/// [`finish`]: UnavailableImages::finish
pub struct UnavailableImages {
    outer: Option<VecSet<String>>,
}

impl UnavailableImages {
    #[must_use]
    pub fn start() -> Self {
        UnavailableImages { outer: UNAVAILABLE.with(|record| record.replace(Some(VecSet::new()))) }
    }

    /// The images recorded since `start`.
    #[must_use]
    pub fn finish(self) -> VecSet<String> {
        UNAVAILABLE.with(|record| record.borrow_mut().take()).unwrap_or_default()
    }
}

impl Drop for UnavailableImages {
    fn drop(&mut self) {
        let outer = self.outer.take();
        UNAVAILABLE.with(|record| *record.borrow_mut() = outer);
    }
}

/// `["image", id]`: the image of an id, and whether it is available.
#[derive(Clone, PartialEq)]
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
        let args_length = value.array_length();
        if args_length < 2 {
            ctx.error("Invalid number of arguments for 'image' expression.".to_owned());
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
        if !available && !evaluated_image_id.is_empty() {
            UNAVAILABLE.with(|record| {
                if let Some(record) = record.borrow_mut().as_mut() {
                    record.insert(evaluated_image_id.clone());
                }
            });
        }
        Ok(Value::Image(Image::with_available(evaluated_image_id, available)))
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        // Port-only: the legacy property functions wrap a token string's
        // image expression in a second one (a C++ quirk the conversion
        // keeps). `["image", ["image", ...]]` doesn't parse back, since
        // "image" takes a string; the outer one evaluates to the inner one's
        // image, so it serializes as the inner one.
        if self.image_id.get_type() == Type::Image {
            return self.image_id.serialize();
        }
        MbglValue::Array(vec![MbglValue::String("image".to_owned()), self.image_id.serialize()])
    }
}
