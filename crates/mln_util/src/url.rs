// Ports src/mln/util/url.hpp and src/mln/util/url.cpp.
//
// Positions are byte offsets, `Segment = (position, length)` as
// `std::pair<size_t, size_t>`. `usize::MAX` is `std::string::npos`; the
// size_t arithmetic wraps as in C++ (`substr` clamps, see `substr`).

use crate::token::replace_tokens;

pub type Segment = (usize, usize);

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

/// `std::string::find(c, pos)`.
fn find(s: &[u8], c: u8, pos: usize) -> usize {
    if pos >= s.len() {
        return NPOS;
    }
    s[pos..].iter().position(|&b| b == c).map_or(NPOS, |i| pos + i)
}

/// `std::string::rfind(c, pos)`.
fn rfind(s: &[u8], c: u8, pos: usize) -> usize {
    if s.is_empty() {
        return NPOS;
    }
    let last = pos.min(s.len() - 1);
    s[..=last].iter().rposition(|&b| b == c).unwrap_or(NPOS)
}

/// `std::string::substr(pos, count)` on bytes (C++ throws for `pos >
/// size()`; callers here never pass that).
pub fn substr(s: &str, pos: usize, count: usize) -> &str {
    let pos = pos.min(s.len());
    let end = pos.saturating_add(count).min(s.len());
    s.get(pos..end).unwrap_or("")
}

/// `util::percentEncode`.
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

/// `util::percentDecode`. Returns bytes: a `%xx` may produce any byte.
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

/// `util::URL`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct URL {
    pub query: Segment,
    pub scheme: Segment,
    pub domain: Segment,
    pub path: Segment,
}

impl URL {
    pub fn new(str_: &str) -> URL {
        let s = str_.as_bytes();
        let query = {
            let hash_pos = find(s, b'#', 0);
            let query_pos = find(s, b'?', 0);
            if query_pos == NPOS || hash_pos < query_pos {
                (if hash_pos != NPOS { hash_pos } else { s.len() }, 0)
            } else {
                (query_pos, (if hash_pos != NPOS { hash_pos } else { s.len() }) - query_pos)
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

/// `util::Path`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Path {
    pub directory: Segment,
    pub extension: Segment,
    pub filename: Segment,
}

impl Path {
    /// `Path(str)` (`pos = 0`, `count = npos`).
    pub fn new(str_: &str) -> Path {
        Path::with_range(str_, 0, NPOS)
    }

    /// `Path(str, pos, count)`.
    pub fn with_range(str_: &str, pos: usize, count: usize) -> Path {
        let s = str_.as_bytes();
        let directory = {
            // Finds the string between pos and the first /, if it exists
            let end_pos = if count == NPOS { s.len() } else { pos + count };
            let slash_pos = rfind(s, b'/', end_pos);
            (pos, if slash_pos == NPOS || slash_pos < pos { 0 } else { slash_pos + 1 - pos })
        };
        let extension = {
            let mut dot_pos = rfind(s, b'.', pos.wrapping_add(count));
            let end_pos = if count == NPOS { s.len() } else { pos + count };
            // Count a preceding @2x to the file extension as well.
            let factor = b"@2x";
            let factor_len = factor.len();
            if dot_pos >= factor_len && dot_pos < end_pos && s.get(dot_pos - factor_len..dot_pos) == Some(&factor[..]) {
                dot_pos -= factor_len;
            }
            if dot_pos == NPOS || dot_pos < directory.0 + directory.1 {
                (end_pos, 0)
            } else {
                (dot_pos, end_pos - dot_pos)
            }
        };
        let filename = {
            let file_pos = directory.0 + directory.1;
            (file_pos, extension.0 - file_pos)
        };
        Path { directory, extension, filename }
    }
}

/// `util::transformURL(tpl, str, url)`.
pub fn transform_url_with(tpl: &str, str_: &str, url: &URL) -> String {
    let mut result = replace_tokens(tpl, |token: &str| -> Option<String> {
        match token {
            "path" => Some(substr(str_, url.path.0, url.path.1).to_owned()),
            "domain" => Some(substr(str_, url.domain.0, url.domain.1).to_owned()),
            "scheme" => Some(substr(str_, url.scheme.0, url.scheme.1).to_owned()),
            "directory" => {
                let path = Path::with_range(str_, url.path.0, url.path.1);
                Some(substr(str_, path.directory.0, path.directory.1).to_owned())
            }
            "filename" => {
                let path = Path::with_range(str_, url.path.0, url.path.1);
                Some(substr(str_, path.filename.0, path.filename.1).to_owned())
            }
            "extension" => {
                let path = Path::with_range(str_, url.path.0, url.path.1);
                Some(substr(str_, path.extension.0, path.extension.1).to_owned())
            }
            _ => None,
        }
    });
    // Append the query string if it exists.
    if url.query.1 > 1 {
        let amp = if result.contains('?') { Some(result.len()) } else { None };
        result.push_str(substr(str_, url.query.0, url.query.1));
        // Transform the question mark to an ampersand if we had a query string previously.
        if let Some(amp) = amp {
            if amp < result.len() {
                result.replace_range(amp..amp + 1, "&");
            }
        }
    }
    result
}

/// `util::transformURL(tpl, url)`.
pub fn transform_url(tpl: &str, url: &str) -> String {
    transform_url_with(tpl, url, &URL::new(url))
}

#[cfg(test)]
mod tests {
    // Ports test/util/url.test.cpp.
    use super::*;

    fn url(s: &str) -> URL {
        URL::new(s)
    }

    fn url_path(s: &str) -> Path {
        let u = URL::new(s);
        Path::with_range(s, u.path.0, u.path.1)
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

    // gtest: Path.Directory
    #[test]
    fn path_directory() {
        let p = Path::new;
        assert_eq!((0, 8), p("foo/bar/baz.ext").directory);
        assert_eq!((0, 8), p("foo.bar/baz.ext").directory);
        assert_eq!((0, 8), p("foo.bar/baz").directory);
        assert_eq!((0, 8), p("foo/bar/.ext").directory);
        assert_eq!((0, 4), p("foo/bar@2x.png").directory);
        assert_eq!((0, 4), p("foo/b").directory);
        assert_eq!((0, 4), p("foo/").directory);
        assert_eq!((0, 0), p("foo").directory);
        assert_eq!((0, 0), p("foo.png").directory);
        assert_eq!((0, 0), p("").directory);

        assert_eq!((0, 9), p("/foo/bar/baz.ext").directory);
        assert_eq!((0, 9), p("/foo.bar/baz.ext").directory);
        assert_eq!((0, 9), p("/foo.bar/baz").directory);
        assert_eq!((0, 9), p("/foo/bar/.ext").directory);
        assert_eq!((0, 5), p("/foo/bar@2x.png").directory);
        assert_eq!((0, 5), p("/foo/b").directory);
        assert_eq!((0, 5), p("/foo/").directory);
        assert_eq!((0, 1), p("/foo").directory);
        assert_eq!((0, 1), p("/foo.png").directory);
        assert_eq!((0, 1), p("/").directory);
    }

    // gtest: Path.URLDirectory
    #[test]
    fn path_url_directory() {
        assert_eq!((18, 9), url_path("http://example.com/foo/bar/baz.ext").directory);
        assert_eq!((18, 9), url_path("http://example.com/foo/bar/baz.ext?query=foo.bar").directory);
        assert_eq!((18, 9), url_path("http://example.com/foo.bar/baz.ext").directory);
        assert_eq!((18, 9), url_path("http://example.com/foo.bar/baz.ext?query=foo.bar").directory);
        assert_eq!((18, 9), url_path("http://example.com/foo.bar/baz").directory);
        assert_eq!((18, 9), url_path("http://example.com/foo.bar/baz?query=foo.bar").directory);
        assert_eq!((18, 9), url_path("http://example.com/foo/bar/.ext").directory);
        assert_eq!((18, 9), url_path("http://example.com/foo/bar/.ext?query=foo.bar").directory);
        assert_eq!((18, 5), url_path("http://example.com/foo/bar@2x.png").directory);
        assert_eq!((18, 5), url_path("http://example.com/foo/bar@2x.png?query=foo.bar").directory);
        assert_eq!((18, 5), url_path("http://example.com/foo/b").directory);
        assert_eq!((18, 5), url_path("http://example.com/foo/b?query=foo.bar").directory);
        assert_eq!((18, 5), url_path("http://example.com/foo/").directory);
        assert_eq!((18, 5), url_path("http://example.com/foo/?query=foo.bar").directory);
        assert_eq!((18, 1), url_path("http://example.com/foo").directory);
        assert_eq!((18, 1), url_path("http://example.com/foo?query=foo.bar").directory);
        assert_eq!((18, 1), url_path("http://example.com/foo.png").directory);
        assert_eq!((18, 1), url_path("http://example.com/foo.png?query=foo.bar").directory);
        assert_eq!((18, 1), url_path("http://example.com/").directory);
        assert_eq!((18, 1), url_path("http://example.com/?query=foo.bar").directory);
        assert_eq!((18, 0), url_path("http://example.com").directory);
        assert_eq!((18, 0), url_path("http://example.com?query=foo.bar").directory);
    }

    // gtest: Path.Extension
    #[test]
    fn path_extension() {
        let p = Path::new;
        assert_eq!((11, 4), p("foo/bar/baz.ext").extension);
        assert_eq!((11, 4), p("foo.bar/baz.ext").extension);
        assert_eq!((18, 4), p("foo.bar/baz.vector.pbf").extension);
        assert_eq!((11, 0), p("foo.bar/baz").extension);
        assert_eq!((8, 4), p("foo/bar/.ext").extension);
        assert_eq!((7, 7), p("foo/bar@2x.png").extension);
        assert_eq!((5, 0), p("foo/b").extension);
        assert_eq!((4, 0), p("foo/").extension);
        assert_eq!((3, 0), p("foo").extension);
        assert_eq!((3, 4), p("foo.png").extension);
        assert_eq!((0, 0), p("").extension);

        assert_eq!((12, 4), p("/foo/bar/baz.ext").extension);
        assert_eq!((12, 4), p("/foo.bar/baz.ext").extension);
        assert_eq!((19, 4), p("/foo.bar/baz.vector.pbf").extension);
        assert_eq!((12, 0), p("/foo.bar/baz").extension);
        assert_eq!((9, 4), p("/foo/bar/.ext").extension);
        assert_eq!((8, 7), p("/foo/bar@2x.png").extension);
        assert_eq!((6, 0), p("/foo/b").extension);
        assert_eq!((5, 0), p("/foo/").extension);
        assert_eq!((4, 0), p("/foo").extension);
        assert_eq!((4, 4), p("/foo.png").extension);
        assert_eq!((1, 0), p("/").extension);
    }

    // gtest: Path.URLExtension
    #[test]
    fn path_url_extension() {
        assert_eq!((30, 4), url_path("http://example.com/foo/bar/baz.ext").extension);
        assert_eq!((30, 4), url_path("http://example.com/foo/bar/baz.ext?query=foo.bar").extension);
        assert_eq!((30, 4), url_path("http://example.com/foo.bar/baz.ext").extension);
        assert_eq!((30, 4), url_path("http://example.com/foo.bar/baz.ext?query=foo.bar").extension);
        assert_eq!((30, 0), url_path("http://example.com/foo.bar/baz").extension);
        assert_eq!((30, 0), url_path("http://example.com/foo.bar/baz?query=foo.bar").extension);
        assert_eq!((27, 4), url_path("http://example.com/foo/bar/.ext").extension);
        assert_eq!((27, 4), url_path("http://example.com/foo/bar/.ext?query=foo.bar").extension);
        assert_eq!((26, 7), url_path("http://example.com/foo/bar@2x.png").extension);
        assert_eq!((26, 7), url_path("http://example.com/foo/bar@2x.png?query=foo.bar").extension);
        assert_eq!((24, 0), url_path("http://example.com/foo/b").extension);
        assert_eq!((24, 0), url_path("http://example.com/foo/b?query=foo.bar").extension);
        assert_eq!((23, 0), url_path("http://example.com/foo/").extension);
        assert_eq!((23, 0), url_path("http://example.com/foo/?query=foo.bar").extension);
        assert_eq!((22, 0), url_path("http://example.com/foo").extension);
        assert_eq!((22, 0), url_path("http://example.com/foo?query=foo.bar").extension);
        assert_eq!((22, 4), url_path("http://example.com/foo.png").extension);
        assert_eq!((22, 4), url_path("http://example.com/foo.png?query=foo.bar").extension);
        assert_eq!((19, 0), url_path("http://example.com/").extension);
        assert_eq!((19, 0), url_path("http://example.com/?query=foo.bar").extension);
        assert_eq!((18, 0), url_path("http://example.com").extension);
        assert_eq!((18, 0), url_path("http://example.com?query=foo.bar").extension);
    }

    // gtest: Path.Filename
    #[test]
    fn path_filename() {
        let p = Path::new;
        assert_eq!((8, 3), p("foo/bar/baz.ext").filename);
        assert_eq!((8, 3), p("foo.bar/baz.ext").filename);
        assert_eq!((8, 3), p("foo.bar/baz").filename);
        assert_eq!((8, 0), p("foo/bar/.ext").filename);
        assert_eq!((4, 3), p("foo/bar@2x.png").filename);
        assert_eq!((4, 1), p("foo/b").filename);
        assert_eq!((4, 0), p("foo/").filename);
        assert_eq!((0, 3), p("foo").filename);
        assert_eq!((0, 3), p("foo.png").filename);
        assert_eq!((0, 0), p("").filename);

        assert_eq!((9, 3), p("/foo/bar/baz.ext").filename);
        assert_eq!((9, 3), p("/foo.bar/baz.ext").filename);
        assert_eq!((9, 3), p("/foo.bar/baz").filename);
        assert_eq!((9, 0), p("/foo/bar/.ext").filename);
        assert_eq!((5, 3), p("/foo/bar@2x.png").filename);
        assert_eq!((5, 1), p("/foo/b").filename);
        assert_eq!((5, 0), p("/foo/").filename);
        assert_eq!((1, 3), p("/foo").filename);
        assert_eq!((1, 3), p("/foo.png").filename);
        assert_eq!((1, 0), p("/").filename);
    }

    // gtest: Path.URLFilename
    #[test]
    fn path_url_filename() {
        assert_eq!((27, 3), url_path("http://example.com/foo/bar/baz.ext").filename);
        assert_eq!((27, 3), url_path("http://example.com/foo/bar/baz.ext?query=foo.bar").filename);
        assert_eq!((27, 3), url_path("http://example.com/foo.bar/baz.ext").filename);
        assert_eq!((27, 3), url_path("http://example.com/foo.bar/baz.ext?query=foo.bar").filename);
        assert_eq!((27, 3), url_path("http://example.com/foo.bar/baz").filename);
        assert_eq!((27, 3), url_path("http://example.com/foo.bar/baz?query=foo.bar").filename);
        assert_eq!((27, 0), url_path("http://example.com/foo/bar/.ext").filename);
        assert_eq!((27, 0), url_path("http://example.com/foo/bar/.ext?query=foo.bar").filename);
        assert_eq!((23, 3), url_path("http://example.com/foo/bar@2x.png").filename);
        assert_eq!((23, 3), url_path("http://example.com/foo/bar@2x.png?query=foo.bar").filename);
        assert_eq!((23, 1), url_path("http://example.com/foo/b").filename);
        assert_eq!((23, 1), url_path("http://example.com/foo/b?query=foo.bar").filename);
        assert_eq!((23, 0), url_path("http://example.com/foo/").filename);
        assert_eq!((23, 0), url_path("http://example.com/foo/?query=foo.bar").filename);
        assert_eq!((19, 3), url_path("http://example.com/foo").filename);
        assert_eq!((19, 3), url_path("http://example.com/foo?query=foo.bar").filename);
        assert_eq!((19, 3), url_path("http://example.com/foo.png").filename);
        assert_eq!((19, 3), url_path("http://example.com/foo.png?query=foo.bar").filename);
        assert_eq!((19, 0), url_path("http://example.com/").filename);
        assert_eq!((19, 0), url_path("http://example.com/?query=foo.bar").filename);
        assert_eq!((18, 0), url_path("http://example.com").filename);
        assert_eq!((18, 0), url_path("http://example.com?query=foo.bar").filename);
    }
}
