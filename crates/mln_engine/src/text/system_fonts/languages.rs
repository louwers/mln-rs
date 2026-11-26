//! The languages to look fonts up for.

/// The user's preferred languages, most preferred first, as BCP 47 tags.
pub(crate) fn preferred() -> Vec<String> {
    // Android's native code can read only the first of the device's
    // languages; Java has the app's.
    #[cfg(target_os = "android")]
    if let Some(languages) = crate::text::system_fonts::android::languages() {
        return languages;
    }
    sys_locale::get_locales().collect()
}

/// The languages to look the font of a CJK character up for, most preferred
/// first: the user's languages that write ideographs. Chinese, Japanese and
/// Korean share many code points but draw them differently, so a Japanese
/// user gets a Japanese font for them. Empty when the user writes none of
/// them, which leaves the choice to the platform.
pub(crate) fn for_ideographs(preferred: &[String]) -> Vec<String> {
    preferred.iter().filter(|tag| writes_ideographs(tag)).cloned().collect()
}

/// Whether the language of `tag` writes CJK ideographs: Chinese (and its
/// languages, like Cantonese), Japanese or Korean.
fn writes_ideographs(tag: &str) -> bool {
    let primary = tag.split(['-', '_']).next().unwrap_or_default().to_ascii_lowercase();
    matches!(primary.as_str(), "zh" | "ja" | "ko" | "yue" | "cmn" | "wuu" | "hak" | "nan" | "gan" | "hsn")
}

#[cfg(test)]
mod tests {
    use super::for_ideographs;

    fn languages(preferred: &[&str]) -> Vec<String> {
        let preferred: Vec<String> = preferred.iter().map(|tag| (*tag).to_owned()).collect();
        for_ideographs(&preferred)
    }

    #[test]
    fn ideographs_take_the_users_cjk_languages_in_order() {
        assert_eq!(languages(&["en-US", "ja-JP", "zh-Hans"]), ["ja-JP", "zh-Hans"]);
        assert_eq!(languages(&["zh_TW", "en"]), ["zh_TW"]);
        assert_eq!(languages(&["en-US", "de"]), Vec::<String>::new());
    }
}
