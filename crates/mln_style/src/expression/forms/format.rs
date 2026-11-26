use feature::{Value as MbglValue, ValueObject};

use crate::color::Color;
use crate::conversion::Convertible;
use crate::expression::formatted::{
    FontStack, Formatted, FormattedSection, K_FORMATTED_SECTION_FONT_SCALE, K_FORMATTED_SECTION_TEXT_COLOR,
    K_FORMATTED_SECTION_TEXT_FONT, K_FORMATTED_SECTION_VERTICAL_ALIGN,
};
use crate::expression::parse::{ParseResult, ParsingContext};
use crate::expression::type_::Type;
use crate::expression::value::{Value, from_expression_value, to_string};
use crate::expression::{EvaluationContext, EvaluationError, EvaluationResult, Expr, Node};
use crate::types::VerticalAlign;

/// A section of a `format` expression: its content and options.
#[derive(Clone, PartialEq, mln_wire::Wire)]
pub struct FormatExpressionSection {
    /// Content can be expression that evaluates to String or Image.
    pub(crate) content: Expr,

    // Text related section options.
    pub(crate) font_scale: Option<Expr>,
    pub(crate) text_font: Option<Expr>,
    pub text_color: Option<Expr>,

    /// A string expression, for text and image sections.
    pub(crate) vertical_align: Option<Expr>,
}

impl FormatExpressionSection {
    #[must_use]
    pub fn new(content: Expr) -> Self {
        FormatExpressionSection { content, font_scale: None, text_font: None, text_color: None, vertical_align: None }
    }

    pub fn set_text_section_options(
        &mut self,
        font_scale: Option<Expr>,
        text_font: Option<Expr>,
        text_color: Option<Expr>,
    ) {
        if let Some(font_scale) = font_scale {
            self.font_scale = Some(font_scale);
        }
        if let Some(text_font) = text_font {
            self.text_font = Some(text_font);
        }
        if let Some(text_color) = text_color {
            self.text_color = Some(text_color);
        }
    }
}

/// Evaluates a section's `vertical-align` option. A name that isn't one of
/// the options aligns to the bottom, as when the option is missing.
fn evaluate_vertical_align(
    vertical_align: Option<&Expr>,
    params: &EvaluationContext<'_>,
) -> Result<Option<VerticalAlign>, EvaluationError> {
    let Some(vertical_align) = vertical_align else {
        return Ok(None);
    };
    Ok(match vertical_align.evaluate(params)? {
        Value::String(name) => name.parse().ok(),
        _ => None,
    })
}

/// `["format", content, options, ...]`.
#[derive(Clone, PartialEq, mln_wire::Wire)]
pub struct FormatExpression {
    sections: Vec<FormatExpressionSection>,
}

impl FormatExpression {
    #[must_use]
    pub fn new_expr(sections: Vec<FormatExpressionSection>) -> Expr {
        Expr::new(Node::Format(Box::new(FormatExpression { sections })))
    }

    #[must_use]
    pub fn get_sections(&self) -> &[FormatExpressionSection] {
        &self.sections
    }

    pub(crate) fn parse(value: &Convertible<'_>, ctx: &mut ParsingContext) -> ParseResult {
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

                let mut vertical_align = None;
                if let Some(option) = arg.object_member(K_FORMATTED_SECTION_VERTICAL_ALIGN) {
                    if let Some(name) = option.to_string()
                        && name.parse::<VerticalAlign>().is_err()
                    {
                        ctx.error(format!(
                            "'vertical-align' must be one of: 'bottom', 'center', 'top' but found '{name}' instead."
                        ));
                        return None;
                    }
                    vertical_align = Some(ctx.parse(&option, 1, Some(Type::String), None)?);
                }

                let Some(section) = sections.last_mut() else {
                    ctx.error("First argument must be an image or text section.".to_owned());
                    return None;
                };
                section.set_text_section_options(font_scale, text_font, text_color);
                section.vertical_align = vertical_align;
            } else {
                let Some(parsed_arg) = ctx.parse(&arg, 1, Some(Type::Value), None) else {
                    ctx.error("Cannot parse formatted section.".to_owned());
                    return None;
                };

                next_token_may_be_object = true;
                sections.push(FormatExpressionSection::new(parsed_arg));
            }
        }

        Some(FormatExpression::new_expr(sections))
    }

    pub(crate) fn evaluate(&self, params: &EvaluationContext<'_>) -> EvaluationResult {
        let mut evaluated_sections = Vec::new();
        for section in &self.sections {
            let content_result = section.content.evaluate(params)?;

            if let Value::Image(image) = &content_result {
                // Omit sections with empty image ids.
                if !image.id().is_empty() {
                    evaluated_sections.push(FormattedSection {
                        vertical_align: evaluate_vertical_align(section.vertical_align.as_ref(), params)?,
                        ..FormattedSection::from_image(image.clone())
                    });
                }
                // Image sections take no text options.
                continue;
            }
            // `toString(Value)` always produces a string.
            let evaluated_text = to_string(&content_result);

            let mut evaluated_font_scale = None;
            if let Some(font_scale) = &section.font_scale {
                let font_scale_result = font_scale.evaluate(params)?;
                let Some(font_scale_value) = font_scale_result.as_number() else {
                    return Err(EvaluationError::type_mismatch(&Type::Number, &font_scale_result));
                };
                evaluated_font_scale = Some(font_scale_value);
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
            evaluated_sections.push(FormattedSection {
                vertical_align: evaluate_vertical_align(section.vertical_align.as_ref(), params)?,
                ..FormattedSection::new(evaluated_text, evaluated_font_scale, evaluated_text_font, evaluated_text_color)
            });
        }
        Ok(Value::Formatted(Formatted::new(evaluated_sections)))
    }

    pub(crate) fn each_child<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        for section in &self.sections {
            visit(&section.content);
            if let Some(font_scale) = &section.font_scale {
                visit(font_scale);
            }
            if let Some(text_font) = &section.text_font {
                visit(text_font);
            }
            if let Some(text_color) = &section.text_color {
                visit(text_color);
            }
            if let Some(vertical_align) = &section.vertical_align {
                visit(vertical_align);
            }
        }
    }

    pub(crate) fn serialize(&self) -> MbglValue {
        let mut serialized = vec![MbglValue::String("format".to_owned())];
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
            if let Some(vertical_align) = &section.vertical_align {
                options.insert(K_FORMATTED_SECTION_VERTICAL_ALIGN.to_owned(), vertical_align.serialize());
            }
            serialized.push(MbglValue::Object(options));
        }
        MbglValue::Array(serialized)
    }
}
