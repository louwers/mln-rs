const TOKEN_RESERVED_CHARS: &[u8] = b"{}";

/// Replaces `{tokens}` in `source` by calling `lookup`; unknown tokens stay
/// as they are.
pub fn replace_tokens(source: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let bytes = source.as_bytes();
    let end = bytes.len();
    let mut result: Vec<u8> = Vec::with_capacity(end);
    let mut pos = 0;
    while pos != end {
        let mut brace = bytes[pos..].iter().position(|&c| c == b'{').map_or(end, |i| pos + i);
        result.extend_from_slice(&bytes[pos..brace]);
        pos = brace;
        if pos != end {
            brace += 1;
            while brace != end && !TOKEN_RESERVED_CHARS.contains(&bytes[brace]) {
                brace += 1;
            }
            if brace != end && bytes[brace] == b'}' {
                let key = &source[pos + 1..brace];
                if let Some(replacement) = lookup(key) {
                    result.extend_from_slice(replacement.as_bytes());
                } else {
                    result.push(b'{');
                    result.extend_from_slice(key.as_bytes());
                    result.push(b'}');
                }
                pos = brace + 1;
            } else {
                result.extend_from_slice(&bytes[pos..brace]);
                pos = brace;
            }
        }
    }
    // Only ASCII braces are inserted or split at, so this stays UTF-8.
    String::from_utf8(result).expect("replace_tokens keeps UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ports test/util/token.test.cpp.
    // gtest: Token.replaceTokens
    #[test]
    fn replace_tokens_test() {
        let one = |key: &'static str, value: &'static str| {
            move |token: &str| Some(if token == key { value.to_owned() } else { String::new() })
        };
        assert_eq!("literal", replace_tokens("literal", one("name", "14th St NW")));
        assert_eq!("14th St NW", replace_tokens("{name}", one("name", "14th St NW")));
        assert_eq!("", replace_tokens("{name}", one("text", "14th St NW")));
        assert_eq!("1400", replace_tokens("{num}", one("num", "1400")));
        assert_eq!("500 m", replace_tokens("{num} m", one("num", "500")));
        assert_eq!(
            "3 Fine Fields",
            replace_tokens("{a} {b} {c}", |token: &str| {
                Some(
                    match token {
                        "a" => "3",
                        "b" => "Fine",
                        "c" => "Fields",
                        _ => "",
                    }
                    .to_owned(),
                )
            })
        );
        assert_eq!(" but still", replace_tokens("{notset} but still", |_: &str| Some(String::new())));
        assert_eq!("dashed", replace_tokens("{dashed-property}", one("dashed-property", "dashed")));
        assert_eq!("colonized", replace_tokens("{colon:property}", one("colon:property", "colonized")));
        assert_eq!("150 m", replace_tokens("{HØYDE} m", one("HØYDE", "150")));
        assert_eq!("{unknown}", replace_tokens("{unknown}", |_: &str| None));
    }
}
