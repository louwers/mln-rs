use mln_expression::formatted::{Formatted, FormattedSection};
use mln_expression::image::Image;

fn image_section(image: Image) -> FormattedSection {
    FormattedSection::from_image(image)
}

fn text_section(text: &str) -> FormattedSection {
    FormattedSection::new(text.to_owned(), None, None, None)
}

// gtest: Formatted.Equality
#[test]
fn equality() {
    let text = Formatted::from_plain("Formatted");
    let empty_image = image_section(Image::new("Formatted"));
    let image = Formatted::new(vec![empty_image]);
    assert!(!(text == image));
    assert_eq!(text, text);
    assert_eq!(image, image);
}

// gtest: Formatted.Empty
#[test]
fn empty() {
    let empty_formatted = Formatted::from_plain("");
    assert!(empty_formatted.empty());

    let empty_text = text_section("");
    let empty_image = image_section(Image::default());
    let multiple_empty_sections = Formatted::new(vec![empty_text.clone(), empty_text.clone(), empty_text.clone()]);
    assert!(multiple_empty_sections.empty());

    let multiple_empty_sections_with_image = Formatted::new(vec![empty_text.clone(), empty_image, empty_text.clone()]);
    assert!(multiple_empty_sections_with_image.empty());

    let text = text_section("Formatted");
    let image = image_section(Image::new("Image"));

    let multiple_sections = Formatted::new(vec![empty_text.clone(), text.clone(), empty_text.clone()]);
    assert!(!multiple_sections.empty());

    let multiple_sections_with_image = Formatted::new(vec![empty_text, image, text]);
    assert!(!multiple_sections_with_image.empty());
}

// gtest: Formatted.ToString
#[test]
fn to_string() {
    let empty_formatted = Formatted::from_plain("");
    assert_eq!(empty_formatted.to_string(), "");

    let text = text_section("Formatted");
    let multiple_sections = Formatted::new(vec![text.clone(), text.clone()]);
    assert_eq!(multiple_sections.to_string(), "FormattedFormatted");

    let image = image_section(Image::new("Image"));
    let multiple_empty_sections_with_image = Formatted::new(vec![text, image]);
    assert_eq!(multiple_empty_sections_with_image.to_string(), "Formatted");
}
