use std::collections::HashMap;

use crate::shaders::wgsl_preprocessor::{get_directive_argument, is_directive, preprocess_wgsl};

fn defines(keys: &[&str]) -> HashMap<String, bool> {
    keys.iter().map(|k| ((*k).to_owned(), true)).collect()
}

// gtest: ShaderGroupPreprocessor.isDirective_ValidDirectives
#[test]
fn is_directive_valid_directives() {
    assert!(is_directive("#ifdef FEATURE", "ifdef"));
    assert!(is_directive("#ifndef FEATURE", "ifndef"));
    assert!(is_directive("#else", "else"));
    assert!(is_directive("#endif", "endif"));

    // With leading whitespace
    assert!(is_directive("  #ifdef FEATURE", "ifdef"));
    assert!(is_directive("\t#ifdef FEATURE", "ifdef"));
    assert!(is_directive("   \t  #ifdef FEATURE", "ifdef"));

    // With whitespace after #
    assert!(is_directive("#  ifdef FEATURE", "ifdef"));
    assert!(is_directive("#\tifdef FEATURE", "ifdef"));
    assert!(is_directive("#   ifdef FEATURE", "ifdef"));
}

// gtest: ShaderGroupPreprocessor.isDirective_InvalidDirectives
#[test]
fn is_directive_invalid_directives() {
    assert!(!is_directive("ifdef FEATURE", "ifdef")); // Missing #
    assert!(!is_directive("#ifdef FEATURE", "ifndef")); // Wrong directive
    assert!(!is_directive("#ifdefine FEATURE", "ifdef")); // Similar but not exact
    assert!(!is_directive("// #ifdef FEATURE", "ifdef")); // Commented out
    assert!(!is_directive("", "ifdef")); // Empty line
    assert!(!is_directive("   ", "ifdef")); // Whitespace only
}

// gtest: ShaderGroupPreprocessor.getDirectiveArgument_ValidArguments
#[test]
fn get_directive_argument_valid_arguments() {
    assert_eq!(get_directive_argument("#ifdef FEATURE"), "FEATURE");
    assert_eq!(get_directive_argument("#ifndef DEBUG_MODE"), "DEBUG_MODE");
    assert_eq!(get_directive_argument("#ifdef  FEATURE"), "FEATURE"); // Extra spaces
    assert_eq!(get_directive_argument("  #ifdef FEATURE"), "FEATURE"); // Leading whitespace
    assert_eq!(get_directive_argument("#ifdef\tFEATURE"), "FEATURE"); // Tab separator
}

// gtest: ShaderGroupPreprocessor.getDirectiveArgument_InvalidOrEmpty
#[test]
fn get_directive_argument_invalid_or_empty() {
    assert_eq!(get_directive_argument(""), ""); // Empty line
    assert_eq!(get_directive_argument("#ifdef"), ""); // No argument
    assert_eq!(get_directive_argument("#else"), ""); // No argument expected
    assert_eq!(get_directive_argument("#endif"), ""); // No argument expected
    assert_eq!(get_directive_argument("no hash symbol"), ""); // Invalid format
}

// gtest: ShaderGroupPreprocessor.getDirectiveArgument_MultipleWords
#[test]
fn get_directive_argument_multiple_words() {
    // Should only return the first word
    assert_eq!(get_directive_argument("#ifdef FEATURE_A FEATURE_B"), "FEATURE_A");
    assert_eq!(get_directive_argument("#ifdef FEATURE\nmore text"), "FEATURE");
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_SimpleIfdef
#[test]
fn preprocess_wgsl_simple_ifdef() {
    let source = "\nline1\n#ifdef ENABLED\nline2\n#endif\nline3\n";
    let result = preprocess_wgsl(source, &defines(&["ENABLED"]));
    assert!(result.contains("line1"));
    assert!(result.contains("line2"));
    assert!(result.contains("line3"));
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_IfdefDisabled
#[test]
fn preprocess_wgsl_ifdef_disabled() {
    // ENABLED is not defined
    let source = "\nline1\n#ifdef ENABLED\nline2\n#endif\nline3\n";
    let result = preprocess_wgsl(source, &defines(&[]));
    assert!(result.contains("line1"));
    assert!(!result.contains("line2")); // Should be excluded
    assert!(result.contains("line3"));
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_Ifndef
#[test]
fn preprocess_wgsl_ifndef() {
    // DISABLED is not defined
    let source = "\nline1\n#ifndef DISABLED\nline2\n#endif\nline3\n";
    let result = preprocess_wgsl(source, &defines(&[]));
    assert!(result.contains("line1"));
    assert!(result.contains("line2")); // Should be included
    assert!(result.contains("line3"));
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_IfndefWithDefine
#[test]
fn preprocess_wgsl_ifndef_with_define() {
    let source = "\nline1\n#ifndef DISABLED\nline2\n#endif\nline3\n";
    let result = preprocess_wgsl(source, &defines(&["DISABLED"]));
    assert!(result.contains("line1"));
    assert!(!result.contains("line2")); // Should be excluded
    assert!(result.contains("line3"));
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_Else
#[test]
fn preprocess_wgsl_else() {
    let source = "\n#ifdef ENABLED\nline1\n#else\nline2\n#endif\n";
    let result = preprocess_wgsl(source, &defines(&["ENABLED"]));
    assert!(result.contains("line1"));
    assert!(!result.contains("line2"));
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_ElseInverted
#[test]
fn preprocess_wgsl_else_inverted() {
    // ENABLED is not defined
    let source = "\n#ifdef ENABLED\nline1\n#else\nline2\n#endif\n";
    let result = preprocess_wgsl(source, &defines(&[]));
    assert!(!result.contains("line1"));
    assert!(result.contains("line2"));
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_NestedConditionals
#[test]
fn preprocess_wgsl_nested_conditionals() {
    let source = "\nline1\n#ifdef OUTER\nline2\n#ifdef INNER\nline3\n#endif\nline4\n#endif\nline5\n";
    let result = preprocess_wgsl(source, &defines(&["OUTER", "INNER"]));
    for line in ["line1", "line2", "line3", "line4", "line5"] {
        assert!(result.contains(line), "{line}");
    }
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_NestedConditionalsPartiallyEnabled
#[test]
fn preprocess_wgsl_nested_conditionals_partially_enabled() {
    // INNER is not defined
    let source = "\nline1\n#ifdef OUTER\nline2\n#ifdef INNER\nline3\n#endif\nline4\n#endif\nline5\n";
    let result = preprocess_wgsl(source, &defines(&["OUTER"]));
    assert!(result.contains("line1"));
    assert!(result.contains("line2"));
    assert!(!result.contains("line3")); // Inner block excluded
    assert!(result.contains("line4"));
    assert!(result.contains("line5"));
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_ComplexNesting
#[test]
fn preprocess_wgsl_complex_nesting() {
    let source = "\n#ifdef A\ninsideA1\n#ifdef B\ninsideB\n#else\nnotB\n#endif\ninsideA2\n#endif\noutside\n";
    let result = preprocess_wgsl(source, &defines(&["A", "B"]));
    assert!(result.contains("insideA1"));
    assert!(result.contains("insideB"));
    assert!(!result.contains("notB"));
    assert!(result.contains("insideA2"));
    assert!(result.contains("outside"));
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_EmptySource
#[test]
fn preprocess_wgsl_empty_source() {
    assert_eq!(preprocess_wgsl("", &defines(&[])), "");
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_NoConditionals
#[test]
fn preprocess_wgsl_no_conditionals() {
    let source = "\nline1\nline2\nline3\n";
    let result = preprocess_wgsl(source, &defines(&[]));
    assert!(result.contains("line1"));
    assert!(result.contains("line2"));
    assert!(result.contains("line3"));
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_PreservesNewlines
#[test]
fn preprocess_wgsl_preserves_newlines() {
    let source = "line1\n#ifdef ENABLED\nline2\n#endif\nline3";
    let result = preprocess_wgsl(source, &defines(&["ENABLED"]));
    // Count newlines - should have 3 (after line1, line2, line3)
    assert_eq!(result.matches('\n').count(), 3);
}

// gtest: ShaderGroupPreprocessor.preprocessWGSL_MismatchedEndif
#[test]
fn preprocess_wgsl_mismatched_endif() {
    // Extra endif should be ignored gracefully
    let source = "\nline1\n#endif\nline2\n";
    let result = preprocess_wgsl(source, &defines(&[]));
    assert!(result.contains("line1"));
    assert!(result.contains("line2"));
}
