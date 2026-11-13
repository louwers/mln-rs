//! Where a redirect goes, for the transports that follow redirects
//! themselves.

/// Whether `url` starts with `http://` or `https://`, in any letter case.
pub(crate) fn has_http_scheme(url: &str) -> bool {
    url.get(..7).is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || url.get(..8).is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
}

/// The URL a redirect from `current` to `location` goes to: `location`
/// resolved against `current` (RFC 3986, section 5.2), or `None` when it
/// isn't an `http` or `https` URL.
pub(crate) fn redirect_url(current: &str, location: &str) -> Option<String> {
    if has_http_scheme(location) {
        return Some(location.to_owned());
    }
    if has_url_scheme(location) || !has_http_scheme(current) {
        return None;
    }

    let scheme_end = current.find("://")?;
    let scheme = &current[..scheme_end];
    if location.starts_with("//") {
        return Some(format!("{scheme}:{location}"));
    }

    let authority_start = scheme_end + 3;
    let origin_end =
        current[authority_start..].find(['/', '?', '#']).map_or(current.len(), |index| authority_start + index);
    let origin = &current[..origin_end];
    let path_end = current.find(['?', '#']).unwrap_or(current.len()).max(origin_end);
    if location.starts_with('/') {
        let (path, suffix) = split_url_path_suffix(location);
        return Some(format!("{origin}{}{suffix}", remove_dot_segments(path)));
    }
    if location.starts_with('?') {
        return Some(format!("{}{location}", &current[..path_end]));
    }
    if location.starts_with('#') {
        let fragment_start = current.find('#').unwrap_or(current.len());
        return Some(format!("{}{location}", &current[..fragment_start]));
    }

    let current_path = &current[origin_end..path_end];
    let directory = current_path.rfind('/').map_or("/", |index| &current_path[..=index]);
    let (path, suffix) = split_url_path_suffix(location);
    Some(format!("{origin}{}{suffix}", remove_dot_segments(&format!("{directory}{path}"))))
}

/// Whether `value` starts with a scheme (a `:` before any `/`, `?` or `#`).
fn has_url_scheme(value: &str) -> bool {
    value.find([':', '/', '?', '#']).is_some_and(|index| value.as_bytes()[index] == b':')
}

/// A URL reference's path, and its query and fragment.
fn split_url_path_suffix(value: &str) -> (&str, &str) {
    value.split_at(value.find(['?', '#']).unwrap_or(value.len()))
}

/// `path` without its `.` and `..` segments.
fn remove_dot_segments(path: &str) -> String {
    if !path.split('/').any(|segment| segment == "." || segment == "..") {
        return path.to_owned();
    }
    let has_trailing_slash = path.ends_with('/') || path.ends_with("/.") || path.ends_with("/..");
    let mut segments = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            segment => segments.push(segment),
        }
    }
    let mut normalized = String::new();
    if path.starts_with('/') {
        normalized.push('/');
    }
    normalized.push_str(&segments.join("/"));
    if (has_trailing_slash && !normalized.ends_with('/')) || normalized.is_empty() {
        normalized.push('/');
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_location_resolves_against_the_current_url() {
        let current = "https://tiles.localhost:8443/styles/v1/style.json?key=1#top";
        let cases = [
            ("https://other.localhost/a", Some("https://other.localhost/a")),
            ("HTTP://other.localhost/a", Some("HTTP://other.localhost/a")),
            ("//cdn.localhost/b", Some("https://cdn.localhost/b")),
            ("/root.json", Some("https://tiles.localhost:8443/root.json")),
            ("/a/../b/./c.json?x#y", Some("https://tiles.localhost:8443/b/c.json?x#y")),
            ("next.json", Some("https://tiles.localhost:8443/styles/v1/next.json")),
            ("../up.json", Some("https://tiles.localhost:8443/styles/up.json")),
            ("?key=2", Some("https://tiles.localhost:8443/styles/v1/style.json?key=2")),
            ("#bottom", Some("https://tiles.localhost:8443/styles/v1/style.json?key=1#bottom")),
            ("ftp://files.localhost/a", None),
            ("mailto:someone@tiles.localhost", None),
        ];
        for (location, expected) in cases {
            assert_eq!(redirect_url(current, location).as_deref(), expected, "{location}");
        }
    }

    #[test]
    fn a_relative_location_needs_an_http_current_url() {
        assert_eq!(redirect_url("file:///a/b.json", "c.json"), None);
        assert_eq!(redirect_url("no-scheme", "/c.json"), None);
        assert_eq!(redirect_url("http://host.localhost", "c.json").as_deref(), Some("http://host.localhost/c.json"));
        assert_eq!(redirect_url("http://host.localhost?q", "c.json").as_deref(), Some("http://host.localhost/c.json"));
    }

    #[test]
    fn dot_segments_are_removed_from_a_path() {
        let cases = [
            ("/a/b/c", "/a/b/c"),
            ("/a/./b", "/a/b"),
            ("/a/../b", "/b"),
            ("/../a", "/a"),
            ("/a/b/..", "/a/"),
            ("/a/.", "/a/"),
            ("/..", "/"),
            ("a/../b", "b"),
            ("/a//b/../c", "/a/c"),
        ];
        for (path, expected) in cases {
            assert_eq!(remove_dot_segments(path), expected, "{path}");
        }
    }
}
