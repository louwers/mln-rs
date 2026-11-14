//! The languages to look each script's font up for.

use crate::scripts::Script;

/// The user's preferred languages, most preferred first, as BCP 47 tags.
pub(crate) fn preferred() -> Vec<String> {
    // Android's native code can read only the first of the device's
    // languages; Java has the app's.
    #[cfg(target_os = "android")]
    if let Some(languages) = crate::android::languages() {
        return languages;
    }
    sys_locale::get_locales().collect()
}

/// The languages to look the font of `script` up for, most preferred first:
/// the script's own language, or for ideographs, the user's languages that
/// write them. Chinese, Japanese and Korean share many code points but draw
/// them differently, so a Japanese user gets a Japanese font for them. Empty
/// when the user writes none of them, which leaves the choice to the
/// platform.
pub(crate) fn for_script(script: &Script, preferred: &[String]) -> Vec<String> {
    match script.language {
        Some(language) => vec![language.to_owned()],
        None => preferred.iter().filter(|tag| writes_ideographs(tag)).cloned().collect(),
    }
}

/// Whether the language of `tag` writes CJK ideographs: Chinese (and its
/// languages, like Cantonese), Japanese or Korean.
fn writes_ideographs(tag: &str) -> bool {
    let primary = tag.split(['-', '_']).next().unwrap_or_default().to_ascii_lowercase();
    matches!(primary.as_str(), "zh" | "ja" | "ko" | "yue" | "cmn" | "wuu" | "hak" | "nan" | "gan" | "hsn")
}

#[cfg(test)]
mod tests {
    use super::for_script;
    use crate::scripts::SCRIPTS;

    fn languages(script: usize, preferred: &[&str]) -> Vec<String> {
        let preferred: Vec<String> = preferred.iter().map(|tag| (*tag).to_owned()).collect();
        for_script(&SCRIPTS[script], &preferred)
    }

    #[test]
    fn ideographs_take_the_users_cjk_languages_in_order() {
        let han = SCRIPTS.iter().position(|script| script.probe == '\u{4E2D}').unwrap();
        assert_eq!(languages(han, &["en-US", "ja-JP", "zh-Hans"]), ["ja-JP", "zh-Hans"]);
        assert_eq!(languages(han, &["zh_TW", "en"]), ["zh_TW"]);
        assert_eq!(languages(han, &["en-US", "de"]), Vec::<String>::new());
    }

    #[test]
    fn other_scripts_take_their_own_language() {
        let devanagari = SCRIPTS.iter().position(|script| script.probe == '\u{0915}').unwrap();
        assert_eq!(languages(devanagari, &["ja-JP"]), ["hi"]);
        let hangul = SCRIPTS.iter().position(|script| script.probe == '\u{D55C}').unwrap();
        assert_eq!(languages(hangul, &["zh-Hans"]), ["ko"]);
    }
}
