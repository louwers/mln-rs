// The std::filesystem operations the C++ runner uses (render-test/filesystem.hpp
// aliases `mln::filesystem` to `std::filesystem`), on paths held as strings in
// their generic format (`path::generic_string()`, `/` between elements): the
// runner computes test ids by string offsets into them, as C++ does. On
// Windows, as std::filesystem there, `\` separates elements too, and a path
// can start with a root name, a drive (`C:`) or a network name (`//server`),
// which an absolute path needs before its root directory. The functions take
// either separator there, and the paths they return are generic.

use std::borrow::Cow;
use std::path::Path;
use std::{fs, io};

/// How paths are read: POSIX's grammar, or Windows', with root names and `\`.
#[derive(Clone, Copy)]
struct Grammar {
    windows: bool,
}

/// This platform's grammar.
const NATIVE: Grammar = Grammar { windows: cfg!(windows) };

impl Grammar {
    fn is_separator(self, c: u8) -> bool {
        c == b'/' || (self.windows && c == b'\\')
    }

    /// `path::generic_string()`.
    fn generic(self, path: &str) -> Cow<'_, str> {
        if self.windows && path.contains('\\') { Cow::Owned(path.replace('\\', "/")) } else { Cow::Borrowed(path) }
    }

    /// The length of the root name that starts `path`: on Windows a drive
    /// (`C:`) or a network name (`//server`), on POSIX none.
    fn root_name_len(self, path: &str) -> usize {
        let bytes = path.as_bytes();
        if !self.windows {
            0
        } else if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            2
        } else if bytes.len() > 2
            && self.is_separator(bytes[0])
            && self.is_separator(bytes[1])
            && !self.is_separator(bytes[2])
        {
            bytes[2..].iter().position(|&c| self.is_separator(c)).map_or(bytes.len(), |i| i + 2)
        } else {
            0
        }
    }

    /// `path::is_relative()`: an absolute path has a root directory, and on
    /// Windows a root name before it.
    fn is_relative(self, path: &str) -> bool {
        let root = self.root_name_len(path);
        let has_root_directory = path.as_bytes().get(root).is_some_and(|&c| self.is_separator(c));
        !(has_root_directory && (root > 0 || !self.windows))
    }

    /// `base / rel` (`path::operator/=`).
    fn join(self, base: &str, rel: &str) -> String {
        let (base, rel) = (self.generic(base), self.generic(rel));
        let (base_root, rel_root) = (self.root_name_len(&base), self.root_name_len(&rel));
        if !self.is_relative(&rel) || (rel_root > 0 && rel[..rel_root] != base[..base_root]) {
            return rel.into_owned();
        }
        let rel = &rel[rel_root..];
        let mut out = if rel.starts_with('/') {
            // A root directory replaces all of `base` but its root name.
            base[..base_root].to_string()
        } else {
            let has_filename = !base[base_root..].is_empty() && !base.ends_with('/');
            let mut out = base.into_owned();
            if has_filename {
                out.push('/');
            }
            out
        };
        out.push_str(rel);
        out
    }

    /// Where `path`'s filename starts: after its last separator, and after
    /// its root name.
    fn filename_start(self, path: &str) -> usize {
        let root = self.root_name_len(path);
        path.as_bytes()[root..].iter().rposition(|&c| self.is_separator(c)).map_or(root, |i| root + i + 1)
    }

    /// `path::filename()`.
    fn filename(self, path: &str) -> &str {
        &path[self.filename_start(path)..]
    }

    /// `path::remove_filename()`.
    fn remove_filename(self, path: &str) -> String {
        self.generic(&path[..self.filename_start(path)]).into_owned()
    }

    /// `path::stem()`.
    fn stem(self, path: &str) -> &str {
        let name = self.filename(path);
        if name == "." || name == ".." {
            return name;
        }
        match name.rfind('.') {
            Some(0) | None => name,
            Some(i) => &name[..i],
        }
    }

    /// `path::lexically_normal()` ([fs.path.generic] normal form).
    fn lexically_normal(self, path: &str) -> String {
        if path.is_empty() {
            return String::new();
        }
        let path = self.generic(path);
        let (root_name, path) = path.split_at(self.root_name_len(&path));
        let absolute = path.starts_with('/');
        let ends_with_separator = path.ends_with('/');
        let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
        let mut elems: Vec<&str> = Vec::new();
        // Whether a directory separator follows the last remaining element.
        let mut trailing = false;
        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            match *part {
                // A dot filename is removed with the separator after it; a final
                // dot leaves the separator before it ("a/." -> "a/").
                "." => {
                    if last {
                        trailing = true;
                    }
                }
                ".." => {
                    if elems.last().is_some_and(|prev| *prev != "..") {
                        // "a/b/.." -> "a/"
                        elems.pop();
                        if last {
                            trailing = true;
                        }
                    } else if absolute {
                        // Dot-dot right after the root directory is removed.
                        if last {
                            trailing = true;
                        }
                    } else {
                        elems.push("..");
                        if last {
                            trailing = ends_with_separator;
                        }
                    }
                }
                name => {
                    elems.push(name);
                    if last {
                        trailing = ends_with_separator;
                    }
                }
            }
        }
        // If the last filename is dot-dot, remove any trailing separator.
        if elems.last() == Some(&"..") {
            trailing = false;
        }
        let mut out = root_name.to_string();
        if absolute {
            out.push('/');
        }
        out.push_str(&elems.join("/"));
        if trailing && !elems.is_empty() {
            out.push('/');
        }
        if out.is_empty() {
            out.push('.');
        }
        out
    }
}

/// `path::is_relative()`.
pub fn is_relative(path: &str) -> bool {
    NATIVE.is_relative(path)
}

/// `base / rel` (`path::operator/=`): an absolute `rel` replaces `base`;
/// otherwise a separator goes between them unless `base` is empty or already
/// ends with one (and on Windows, `rel`'s root directory keeps only `base`'s
/// root name).
pub fn join(base: &str, rel: &str) -> String {
    NATIVE.join(base, rel)
}

/// `path::filename()`: the part after the last separator (empty for `"a/"`).
pub fn filename(path: &str) -> &str {
    NATIVE.filename(path)
}

/// `path::remove_filename()`: keeps the trailing separator.
pub fn remove_filename(path: &str) -> String {
    NATIVE.remove_filename(path)
}

/// `path::stem()`.
pub fn stem(path: &str) -> &str {
    NATIVE.stem(path)
}

/// `std::filesystem::exists` (follows symlinks).
pub fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// `path::lexically_normal()` ([fs.path.generic] normal form).
pub fn lexically_normal(path: &str) -> String {
    NATIVE.lexically_normal(path)
}

/// The entries of `std::filesystem::directory_iterator(dir)`, as
/// `dir / filename` strings, in `readdir` order (libstdc++ does not sort
/// either).
pub fn directory_entries(dir: &str) -> io::Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        out.push(join(dir, &entry.file_name().to_string_lossy()));
    }
    Ok(out)
}

/// The entries of `std::filesystem::recursive_directory_iterator(root)`:
/// pre-order, `readdir` order, descending into directories but not into
/// symlinks to directories (no `follow_directory_symlink`).
pub fn recursive_directory_entries(root: &str) -> io::Result<Vec<String>> {
    fn walk(dir: &str, out: &mut Vec<String>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = join(dir, &entry.file_name().to_string_lossy());
            let is_dir = entry.file_type()?.is_dir();
            out.push(path.clone());
            if is_dir {
                walk(&path, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, &mut out)?;
    Ok(out)
}

/// `directory_entry::is_regular_file()` (follows symlinks).
pub fn is_regular_file(path: &str) -> bool {
    fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POSIX: Grammar = Grammar { windows: false };
    const WINDOWS: Grammar = Grammar { windows: true };

    #[test]
    fn lexically_normal_matches_std_filesystem() {
        // Examples from [fs.path.generic] / cppreference, plus the shapes the
        // manifests produce, the same in either grammar.
        let cases = [
            ("", ""),
            ("foo/./bar/..", "foo/"),
            ("foo/.///bar/../", "foo/"),
            ("a/b/../../..", ".."),
            ("/../a", "/a"),
            ("/..", "/"),
            ("/a/..", "/"),
            ("./", "."),
            (".", "."),
            ("a/..", "."),
            ("a/.", "a/"),
            ("a/./b", "a/b"),
            ("../a/", "../a/"),
            ("../a/..", ".."),
            ("a//b", "a/b"),
            ("//", "/"),
            ("a/b/../", "a/"),
            (
                "tools/render-test/manifests/../../../../maplibre+/metrics/integration",
                "../maplibre+/metrics/integration",
            ),
            ("../maplibre+/metrics/./", "../maplibre+/metrics/"),
            ("/x/y/../z", "/x/z"),
        ];
        for grammar in [POSIX, WINDOWS] {
            for (input, expected) in cases {
                assert_eq!(grammar.lexically_normal(input), expected, "{input:?}");
            }
        }
    }

    #[test]
    fn join_and_names() {
        for grammar in [POSIX, WINDOWS] {
            assert_eq!(grammar.join("a", "b"), "a/b");
            assert_eq!(grammar.join("a/", "b"), "a/b");
            assert_eq!(grammar.join("", "b"), "b");
            assert_eq!(grammar.join("a", ""), "a/");
            assert_eq!(grammar.join("a", "/b"), "/b");
            assert_eq!(grammar.filename("a/b/style.json"), "style.json");
            assert_eq!(grammar.filename("a/b/"), "");
            assert_eq!(grammar.remove_filename("a/b/style.json"), "a/b/");
            assert_eq!(grammar.remove_filename("style.json"), "");
            assert_eq!(grammar.stem("x/linux-wgpu.json"), "linux-wgpu");
            assert_eq!(grammar.stem("x/.hidden"), ".hidden");
            assert!(grammar.is_relative("a/b"));
        }
        assert!(!POSIX.is_relative("/a"));
        // POSIX file names can hold `\`.
        assert_eq!(POSIX.filename(r"a\b"), r"a\b");
        assert!(POSIX.is_relative("C:/a"));
    }

    #[test]
    fn windows_paths_have_root_names_and_take_either_separator() {
        // As MSVC's std::filesystem: absolute takes a root name and a root
        // directory, and a root directory alone keeps `base`'s root name.
        assert!(!WINDOWS.is_relative("C:/a") && !WINDOWS.is_relative(r"C:\a") && !WINDOWS.is_relative(r"\\server\a"));
        assert!(WINDOWS.is_relative("/a") && WINDOWS.is_relative(r"\a") && WINDOWS.is_relative("C:a"));
        assert_eq!(WINDOWS.join(r"C:\a", "b"), "C:/a/b");
        assert_eq!(WINDOWS.join(r"C:\a\", r"b\c"), "C:/a/b/c");
        assert_eq!(WINDOWS.join("C:/a", "/b"), "C:/b");
        assert_eq!(WINDOWS.join("C:/a", r"D:\b"), "D:/b");
        assert_eq!(WINDOWS.join("C:", "b"), "C:b");
        assert_eq!(WINDOWS.join("x", r"C:\a"), "C:/a");
        assert_eq!(WINDOWS.filename(r"C:\a\style.json"), "style.json");
        assert_eq!(WINDOWS.filename("C:style.json"), "style.json");
        assert_eq!(WINDOWS.filename("C:"), "");
        assert_eq!(WINDOWS.filename(r"\\server"), "");
        assert_eq!(WINDOWS.remove_filename(r"C:\a\style.json"), "C:/a/");
        assert_eq!(WINDOWS.stem(r"C:\a\linux-wgpu.json"), "linux-wgpu");
        assert_eq!(WINDOWS.lexically_normal(r"C:\a\.\b\..\c\"), "C:/a/c/");
        assert_eq!(WINDOWS.lexically_normal("C:/.."), "C:/");
        assert_eq!(WINDOWS.lexically_normal("C:"), "C:");
        assert_eq!(WINDOWS.lexically_normal("C:a/.."), "C:");
        assert_eq!(WINDOWS.lexically_normal(r"\\server\share\..\x"), "//server/x");
    }
}
