// Positions are byte offsets, `Segment = (position, length)` as
// `std::pair<size_t, size_t>`. `usize::MAX` is `std::string::npos`; the
// size_t arithmetic wraps as in C++ (`substr` clamps, see `substr`).

pub(crate) type Segment = (usize, usize);

const NPOS: usize = usize::MAX;

// std::alnum etc. suffer from locale-dependence.
fn is_alpha_character(c: u8) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_uppercase()
}

fn is_alpha_numeric_character(c: u8) -> bool {
    is_alpha_character(c) || c.is_ascii_digit()
}

fn is_scheme_character(c: u8) -> bool {
    is_alpha_numeric_character(c) || c == b'-' || c == b'+' || c == b'.'
}

fn to_lower_hex(c: u8) -> u8 {
    let c = c & 0x0F;
    b'0' + c + if c > 9 { 39 } else { 0 }
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn find(s: &[u8], c: u8, pos: usize) -> usize {
    if pos >= s.len() {
        return NPOS;
    }
    s[pos..].iter().position(|&b| b == c).map_or(NPOS, |i| pos + i)
}

/// Up to `count` bytes of `s` from `pos`, clamped to the end of `s`. Empty
/// if either end falls inside a UTF-8 character.
#[must_use]
pub fn substr(s: &str, pos: usize, count: usize) -> &str {
    let pos = pos.min(s.len());
    let end = pos.saturating_add(count).min(s.len());
    s.get(pos..end).unwrap_or("")
}

#[must_use]
pub fn percent_encode(input: &str) -> String {
    let mut encoded = String::new();
    for &c in input.as_bytes() {
        if is_alpha_numeric_character(c) || c == b'-' || c == b'_' || c == b'.' || c == b'~' {
            encoded.push(c as char);
        } else {
            encoded.push('%');
            encoded.push(to_lower_hex(c >> 4) as char);
            encoded.push(to_lower_hex(c) as char);
        }
    }
    encoded
}

/// Decodes `%xx` escapes. Returns bytes: a `%xx` may produce any byte.
#[must_use]
pub fn percent_decode(input: &str) -> Vec<u8> {
    let bytes = input.as_bytes();
    let mut decoded = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // A % that does not start two hex digits stays literal, as in the WHATWG URL Standard.
        let high = if bytes[i] == b'%' && i + 2 < bytes.len() { hex_value(bytes[i + 1]) } else { None };
        let low = high.and_then(|_| hex_value(bytes[i + 2]));
        if let (Some(high), Some(low)) = (high, low) {
            decoded.push(high * 16 + low);
            i += 2;
        } else {
            decoded.push(bytes[i]);
        }
        i += 1;
    }
    decoded
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct URL {
    pub query: Segment,
    pub(crate) scheme: Segment,
    pub(crate) domain: Segment,
    pub path: Segment,
}

impl URL {
    #[must_use]
    pub fn new(str_: &str) -> URL {
        let s = str_.as_bytes();
        let query = {
            let hash_pos = find(s, b'#', 0);
            let query_pos = find(s, b'?', 0);
            if query_pos == NPOS || hash_pos < query_pos {
                (if hash_pos == NPOS { s.len() } else { hash_pos }, 0)
            } else {
                (query_pos, if hash_pos == NPOS { s.len() } else { hash_pos } - query_pos)
            }
        };
        let scheme = {
            if s.is_empty() || !is_alpha_character(s[0]) {
                (0, 0)
            } else {
                let mut scheme_end = 0;
                while scheme_end < query.0 && is_scheme_character(s[scheme_end]) {
                    scheme_end += 1;
                }
                // `str[schemeEnd]` is '\0' at the end of the string.
                (0, if s.get(scheme_end) == Some(&b':') { scheme_end } else { 0 })
            }
        };
        let is_data = &s[scheme.0..scheme.0 + scheme.1] == b"data";
        let domain = {
            let mut domain_pos = scheme.0 + scheme.1;
            while domain_pos < query.0 && (s[domain_pos] == b':' || s[domain_pos] == b'/') {
                domain_pos += 1;
            }
            let end_pos = find(s, if is_data { b',' } else { b'/' }, domain_pos);
            (domain_pos, query.0.min(end_pos).wrapping_sub(domain_pos))
        };
        let path = {
            let mut path_pos = domain.0.wrapping_add(domain.1);
            if is_data {
                // Skip comma
                path_pos = path_pos.wrapping_add(1);
            }
            (path_pos, query.0.wrapping_sub(path_pos))
        };
        URL { query, scheme, domain, path }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> URL {
        URL::new(s)
    }

    // gtest: URL.percentEncode
    #[test]
    fn percent_encode_test() {
        assert_eq!("%22%c3%a9nc%c3%b8%c3%b0ing%22", percent_encode("\"éncøðing\""));
    }

    // gtest: URL.percentDecode
    #[test]
    fn percent_decode_test() {
        assert_eq!("\"éncøðing\"".as_bytes(), percent_decode("%22%C3%A9nc%C3%B8%C3%B0ing%22"));
        assert_eq!(b"100%", percent_decode("100%").as_slice());
        assert_eq!(b"%//x", percent_decode("%//x").as_slice());
        assert_eq!(b"A%4", percent_decode("%41%4").as_slice());
    }

    // gtest: URL.Scheme
    #[test]
    fn scheme() {
        assert_eq!((0, 4), url("http://example.com/test?query=foo").scheme);
        assert_eq!((0, 4), url("http://127.0.0.1:8080/test?query=foo").scheme);
        assert_eq!((0, 4), url("http://[2a01:4f8:c17:3680::386a:6f3d]:8080/test?query=foo").scheme);
        assert_eq!((0, 4), url("http://user:password@example.com.:80/test?query=foo").scheme);
        assert_eq!((0, 0), url("htt").scheme);
        assert_eq!((0, 6), url("mapbox://").scheme);
        assert_eq!((0, 6), url("mapbox:/#").scheme);
        assert_eq!((0, 0), url("://").scheme);
        assert_eq!((0, 0), url("").scheme);
        assert_eq!((0, 0), url("http?query://baz").scheme);
        assert_eq!((0, 0), url(":::").scheme);
        assert_eq!((0, 0), url("127.0.0.1:8080/test?query=foo").scheme);
        assert_eq!((0, 0), url("[2a01:4f8:c17:3680::386a:6f3d]:8080/test?query=foo").scheme);
        assert_eq!((0, 4), url("data:,Hello%2C%20World!").scheme);
        assert_eq!((0, 4), url("data:text/plain;base64,SGVsbG8sIFdvcmxkIQ%3D%3D").scheme);
    }

    // gtest: URL.Query
    #[test]
    fn query() {
        assert_eq!((23, 10), url("http://example.com/test?query=foo").query);
        assert_eq!((26, 10), url("http://127.0.0.1:8080/test?query=foo").query);
        assert_eq!((47, 10), url("http://[2a01:4f8:c17:3680::386a:6f3d]:8080/test?query=foo").query);
        assert_eq!((41, 10), url("http://user:password@example.com.:80/test?query=foo").query);
        assert_eq!((23, 10), url("http://example.com/test?query=foo#page-2").query);
        assert_eq!((23, 0), url("http://example.com/test#query=foo?page-2").query);
        assert_eq!((0, 10), url("?query=foo").query);
        assert_eq!((0, 10), url("?query=foo#bar").query);
        assert_eq!((0, 0), url("#?query=foo#bar").query);
        assert_eq!((4, 12), url("http?query://baz").query);
        assert_eq!((9, 4), url("mapbox://?bar").query);
        assert_eq!((12, 0), url("mapbox://bar").query);
        assert_eq!((0, 0), url("").query);
        assert_eq!((3, 0), url(":::").query);
        assert_eq!((19, 10), url("127.0.0.1:8080/test?query=foo").query);
        assert_eq!((40, 10), url("[2a01:4f8:c17:3680::386a:6f3d]:8080/test?query=foo").query);
        assert_eq!((23, 0), url("data:,Hello%2C%20World!").query);
        assert_eq!((47, 0), url("data:text/plain;base64,SGVsbG8sIFdvcmxkIQ%3D%3D").query);
    }

    // gtest: URL.Domain
    #[test]
    fn domain() {
        assert_eq!((7, 11), url("http://example.com/test?query=foo").domain);
        assert_eq!((7, 14), url("http://127.0.0.1:8080/test?query=foo").domain);
        assert_eq!((7, 35), url("http://[2a01:4f8:c17:3680::386a:6f3d]:8080/test?query=foo").domain);
        assert_eq!((7, 29), url("http://user:password@example.com.:80/test?query=foo").domain);
        assert_eq!((5, 11), url("http:example.com/test?query=foo").domain);
        assert_eq!((0, 3), url("htt").domain);
        assert_eq!((0, 4), url("http?query://baz").domain);
        assert_eq!((0, 6), url("domain/foo?bar").domain);
        assert_eq!((7, 0), url("http://").domain);
        assert_eq!((7, 6), url("http://domain").domain);
        assert_eq!((7, 6), url("http://domain#bar").domain);
        assert_eq!((7, 6), url("http://domain/").domain);
        assert_eq!((7, 6), url("http://domain/foo").domain);
        assert_eq!((7, 6), url("http://domain/foo?").domain);
        assert_eq!((0, 1), url("h##p://domain/foo?bar").domain);
        assert_eq!((7, 6), url("http://domain/foo?bar").domain);
        assert_eq!((7, 6), url("http://domain/?").domain);
        assert_eq!((7, 6), url("http://domain?").domain);
        assert_eq!((7, 6), url("http://domain?foo").domain);
        assert_eq!((3, 0), url(":::").domain);
        assert_eq!((0, 14), url("127.0.0.1:8080/test?query=foo").domain);
        assert_eq!((0, 35), url("[2a01:4f8:c17:3680::386a:6f3d]:8080/test?query=foo").domain);
        assert_eq!((5, 0), url("data:,Hello%2C%20World!").domain);
        assert_eq!((5, 17), url("data:text/plain;base64,SGVsbG8sIFdvcmxkIQ%3D%3D").domain);
    }

    // gtest: URL.Path
    #[test]
    fn path() {
        assert_eq!((18, 5), url("http://example.com/test?query=foo").path);
        assert_eq!((21, 5), url("http://127.0.0.1:8080/test?query=foo").path);
        assert_eq!((42, 5), url("http://[2a01:4f8:c17:3680::386a:6f3d]:8080/test?query=foo").path);
        assert_eq!((36, 5), url("http://user:password@example.com.:80/test?query=foo").path);
        assert_eq!((18, 5), url("http://example.com/test?query=foo#bar").path);
        assert_eq!((18, 5), url("http://example.com/test#bar").path);
        assert_eq!((18, 0), url("http://example.com?query=foo").path);
        assert_eq!((18, 0), url("http://example.com#?query=foo").path);
        assert_eq!((18, 1), url("http://example.com/?query=foo").path);
        assert_eq!((3, 0), url(":::").path);
        assert_eq!((14, 5), url("127.0.0.1:8080/test?query=foo").path);
        assert_eq!((35, 5), url("[2a01:4f8:c17:3680::386a:6f3d]:8080/test?query=foo").path);
        assert_eq!((13, 0), url("http://domain").path);
        assert_eq!((6, 4), url("domain/foo?bar").path);
        assert_eq!((6, 17), url("data:,Hello%2C%20World!").path);
        assert_eq!((23, 24), url("data:text/plain;base64,SGVsbG8sIFdvcmxkIQ%3D%3D").path);
    }
}
