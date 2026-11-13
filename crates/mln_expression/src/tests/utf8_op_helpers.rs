use crate::utf8_op_helpers::{get_unicode_character_offset_on_validated_utf8, unicode_length_on_validated_utf8};

// gtest: Utf8OpHelpers.unicodeLengthOnValidatedUtf8
#[test]
fn unicode_length_on_validated_utf8_test() {
    assert_eq!(unicode_length_on_validated_utf8(""), 0);
    assert_eq!(unicode_length_on_validated_utf8("Hello, world!"), 13);
    assert_eq!(unicode_length_on_validated_utf8("aäü"), 3);
    assert_eq!(unicode_length_on_validated_utf8("オオオ"), 3);
    assert_eq!(unicode_length_on_validated_utf8("𝄆𝄆"), 2);
    assert_eq!(unicode_length_on_validated_utf8("aöオ𝄆オöa"), 7);
}

// gtest: Utf8OpHelpers.getUnicodeCharacterOffsetOnValidatedUtf8
#[test]
fn get_unicode_character_offset_on_validated_utf8_test() {
    // edge case: empty string
    assert_eq!(get_unicode_character_offset_on_validated_utf8("", 0), 0);

    let s = "aöオ𝄆オöa";
    assert_eq!(get_unicode_character_offset_on_validated_utf8(s, 0), 0); // 'a'
    assert_eq!(get_unicode_character_offset_on_validated_utf8(s, 1), 1); // 'ö'
    assert_eq!(get_unicode_character_offset_on_validated_utf8(s, 2), 3); // 'オ'
    assert_eq!(get_unicode_character_offset_on_validated_utf8(s, 3), 6); // '𝄆'
    assert_eq!(get_unicode_character_offset_on_validated_utf8(s, 4), 10); // 'オ'
    assert_eq!(get_unicode_character_offset_on_validated_utf8(s, 5), 13); // 'ö'
    assert_eq!(get_unicode_character_offset_on_validated_utf8(s, 6), 15); // 'a'

    // Test out of bounds character offset (should return the string length)
    assert_eq!(get_unicode_character_offset_on_validated_utf8(s, 7), 16);
    assert_eq!(get_unicode_character_offset_on_validated_utf8(s, 12), 16);
    assert_eq!(get_unicode_character_offset_on_validated_utf8(s, 50000), 16);
}
