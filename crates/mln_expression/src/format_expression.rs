use std::sync::Arc;

use feature::{Value as MbglValue, ValueObject};
use mln_conversion::Convertible;
use mln_util::Color;

use crate::expression::{
    Dependency, EvaluationContext, EvaluationError, EvaluationResult, Expression, ExpressionBase, Kind, downcast,
};
use crate::formatted::{
    FontStack, Formatted, FormattedSection, K_FORMATTED_SECTION_FONT_SCALE, K_FORMATTED_SECTION_TEXT_COLOR,
    K_FORMATTED_SECTION_TEXT_FONT,
};
use crate::parsing_context::{ParseResult, ParsingContext};
use crate::type_::Type;
use crate::value::{Value, from_expression_value, to_string, type_of};

#[derive(Clone)]
pub struct FormatExpressionSection {
    /// Content can be expression that evaluates to String or Image.
    pub content: Arc<dyn Expression>,

    // Text related section options.
    pub font_scale: Option<Arc<dyn Expression>>,
    pub text_font: Option<Arc<dyn Expression>>,
    pub text_color: Option<Arc<dyn Expression>>,
}

impl FormatExpressionSection {
    #[must_use]
    pub fn new(content: Box<dyn Expression>) -> Self {
        FormatExpressionSection { content: Arc::from(content), font_scale: None, text_font: None, text_color: None }
    }

    pub fn set_text_section_options(
        &mut self,
        font_scale: Option<Box<dyn Expression>>,
        text_font: Option<Box<dyn Expression>>,
        text_color: Option<Box<dyn Expression>>,
    ) {
        if let Some(font_scale) = font_scale {
            self.font_scale = Some(Arc::from(font_scale));
        }
        if let Some(text_font) = text_font {
            self.text_font = Some(Arc::from(text_font));
        }
        if let Some(text_color) = text_color {
            self.text_color = Some(Arc::from(text_color));
        }
    }

    fn deps(&self) -> Dependency {
        let opt = |e: &Option<Arc<dyn Expression>>| e.as_ref().map_or(Dependency::NONE, |e| e.dependencies());
        self.content.dependencies() | opt(&self.font_scale) | opt(&self.text_font) | opt(&self.text_color)
    }
}

pub struct FormatExpression {
    base: ExpressionBase,
    sections: Vec<FormatExpressionSection>,
}

fn opt_equal(l: Option<&dyn Expression>, r: Option<&dyn Expression>) -> bool {
    match (l, r) {
        (Some(l), Some(r)) => l.equals(r),
        (None, None) => true,
        _ => false,
    }
}

impl FormatExpression {
    #[must_use]
    pub fn new(sections: Vec<FormatExpressionSection>) -> Self {
        let deps = sections.iter().fold(Dependency::NONE, |a, s| a | s.deps());
        FormatExpression { base: ExpressionBase::new(Kind::FormatExpression, Type::Formatted, deps), sections }
    }

    #[must_use]
    pub fn get_sections(&self) -> &[FormatExpressionSection] {
        &self.sections
    }

    pub fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
        let args_length = value.array_length();
        if args_length < 2 {
            ctx.error("Expected at least one argument.".to_owned());
            return None;
        }

        if value.array_member(1).is_object() {
            ctx.error("First argument must be an image or text section.".to_owned());
            return None;
        }

        let mut sections: Vec<FormatExpressionSection> = Vec::new();
        let mut next_token_may_be_object = false;
        for i in 1..args_length {
            let arg = value.array_member(i);

            if next_token_may_be_object && arg.is_object() {
                next_token_may_be_object = false;

                let mut font_scale = None;
                if let Some(option) = arg.object_member(K_FORMATTED_SECTION_FONT_SCALE) {
                    font_scale = Some(ctx.parse(&option, 1, Some(Type::Number), None)?);
                }

                let mut text_font = None;
                if let Some(option) = arg.object_member(K_FORMATTED_SECTION_TEXT_FONT) {
                    text_font = Some(ctx.parse(&option, 1, Some(Type::array(Type::String)), None)?);
                }

                let mut text_color = None;
                if let Some(option) = arg.object_member(K_FORMATTED_SECTION_TEXT_COLOR) {
                    text_color = Some(ctx.parse(&option, 1, Some(Type::Color), None)?);
                }

                sections
                    .last_mut()
                    .expect("a section before its options")
                    .set_text_section_options(font_scale, text_font, text_color);
            } else {
                let Some(parsed_arg) = ctx.parse(&arg, 1, Some(Type::Value), None) else {
                    ctx.error("Cannot parse formatted section.".to_owned());
                    return None;
                };

                next_token_may_be_object = true;
                sections.push(FormatExpressionSection::new(parsed_arg));
            }
        }

        Some(Box::new(FormatExpression::new(sections)))
    }
}

impl Expression for FormatExpression {
    fn base(&self) -> &ExpressionBase {
        &self.base
    }

    fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let mut evaluated_sections = Vec::new();
        for section in &self.sections {
            let content_result = section.content.evaluate(params)?;

            if type_of(&content_result) == Type::Image {
                let Value::Image(image) = &content_result else { unreachable!() };
                // Omit sections with empty image ids.
                if !image.id().is_empty() {
                    evaluated_sections.push(FormattedSection::from_image(image.clone()));
                }
                // Continue evaluation of a next section, as the image section
                // does not have section options.
                continue;
            }
            // `toString(Value)` always produces a string.
            let evaluated_text = to_string(&content_result);

            let mut evaluated_font_scale = None;
            if let Some(font_scale) = &section.font_scale {
                let font_scale_result = font_scale.evaluate(params)?;
                evaluated_font_scale = Some(font_scale_result.as_number().expect("numeric font-scale"));
            }

            let mut evaluated_text_font: Option<FontStack> = None;
            if let Some(text_font) = &section.text_font {
                let text_font_result = text_font.evaluate(params)?;
                let Some(text_font_value) = from_expression_value::<Vec<String>>(&text_font_result) else {
                    return Err(EvaluationError::new("Format text-font option must evaluate to an array of strings"));
                };
                evaluated_text_font = Some(text_font_value);
            }

            let mut evaluated_text_color = None;
            if let Some(text_color) = &section.text_color {
                let text_color_result = text_color.evaluate(params)?;

                evaluated_text_color = from_expression_value::<Color>(&text_color_result);
                if evaluated_text_color.is_none() {
                    return Err(EvaluationError::new("Format text-color option must evaluate to Color"));
                }
            }
            evaluated_sections.push(FormattedSection::new(
                evaluated_text,
                evaluated_font_scale,
                evaluated_text_font,
                evaluated_text_color,
            ));
        }
        Ok(Value::Formatted(Formatted::new(evaluated_sections)))
    }

    fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a dyn Expression)) {
        for section in &self.sections {
            visit(&*section.content);
            if let Some(font_scale) = &section.font_scale {
                visit(&**font_scale);
            }
            if let Some(text_font) = &section.text_font {
                visit(&**text_font);
            }
            if let Some(text_color) = &section.text_color {
                visit(&**text_color);
            }
        }
    }

    fn equals(&self, e: &dyn Expression) -> bool {
        if e.get_kind() == Kind::FormatExpression {
            let rhs = downcast::<FormatExpression>(e).expect("FormatExpression");
            if self.sections.len() != rhs.sections.len() {
                return false;
            }
            for (lhs_section, rhs_section) in self.sections.iter().zip(&rhs.sections) {
                if !lhs_section.content.equals(&*rhs_section.content) {
                    return false;
                }
                if !opt_equal(lhs_section.font_scale.as_deref(), rhs_section.font_scale.as_deref())
                    || !opt_equal(lhs_section.text_font.as_deref(), rhs_section.text_font.as_deref())
                    || !opt_equal(lhs_section.text_color.as_deref(), rhs_section.text_color.as_deref())
                {
                    return false;
                }
            }
            return true;
        }
        false
    }

    fn possible_outputs(&self) -> Vec<Option<Value>> {
        // Technically the combinatoric set of all children
        // Usually, this.text will be undefined anyway
        vec![None]
    }

    fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String(self.get_operator())];
        for section in &self.sections {
            serialized.push(section.content.serialize());
            let mut options = ValueObject::new();
            if let Some(font_scale) = &section.font_scale {
                options.insert(K_FORMATTED_SECTION_FONT_SCALE.to_owned(), font_scale.serialize());
            }
            if let Some(text_font) = &section.text_font {
                options.insert(K_FORMATTED_SECTION_TEXT_FONT.to_owned(), text_font.serialize());
            }
            if let Some(text_color) = &section.text_color {
                options.insert(K_FORMATTED_SECTION_TEXT_COLOR.to_owned(), text_color.serialize());
            }
            serialized.push(MbglValue::Object(options));
        }
        MbglValue::Array(serialized)
    }

    fn get_operator(&self) -> String {
        "format".to_owned()
    }
}
