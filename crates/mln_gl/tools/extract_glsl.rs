// Build tool: extracts the GLSL sources from the C++ headers
// include/mln/shaders/gl/*.hpp into a Rust module, unchanged.
//
// Usage: extract_glsl <out.rs> <header.hpp>...
//
// For every `struct ShaderSource<BuiltIn::X, gfx::Backend::Type::OpenGL>`
// it collects the `name`, `vertex` and `fragment` members, which are C++
// string literal sequences (`R"(...)"`, adjacent literals concatenate). The
// output defines one `pub const X: GlslShaderSource` per specialisation plus
// `ALL`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Punct(char),
}

fn tokenize(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let mut line_start = true;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line_start = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Preprocessor lines
        if line_start && c == b'#' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        line_start = false;
        // Comments
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        // Raw string literal R"delim( ... )delim"
        if c == b'R' && i + 1 < b.len() && b[i + 1] == b'"' {
            let open = i + 2;
            let paren = src[open..].find('(').expect("raw string without '('") + open;
            let delim = &src[open..paren];
            let close = format!("){delim}\"");
            let body_start = paren + 1;
            let end = src[body_start..].find(&close).expect("unterminated raw string") + body_start;
            out.push(Tok::Str(src[body_start..end].to_owned()));
            i = end + close.len();
            continue;
        }
        // Ordinary string literal
        if c == b'"' {
            let mut s = String::new();
            i += 1;
            while i < b.len() && b[i] != b'"' {
                if b[i] == b'\\' && i + 1 < b.len() {
                    i += 1;
                    s.push(match b[i] {
                        b'n' => '\n',
                        b't' => '\t',
                        b'r' => '\r',
                        b'0' => '\0',
                        other => other as char,
                    });
                    i += 1;
                } else {
                    let ch = src[i..].chars().next().unwrap();
                    s.push(ch);
                    i += ch.len_utf8();
                }
            }
            i += 1;
            out.push(Tok::Str(s));
            continue;
        }
        // Character literal (skip)
        if c == b'\'' {
            i += 1;
            while i < b.len() && b[i] != b'\'' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if c.is_ascii_alphanumeric() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Ident(src[start..i].to_owned()));
            continue;
        }
        out.push(Tok::Punct(c as char));
        i += 1;
    }
    out
}

#[derive(Default, Debug)]
struct Source {
    built_in: String,
    name: Option<String>,
    vertex: Option<String>,
    fragment: Option<String>,
}

/// The value of `= <expr> ;` starting at `toks[i]`, if it is a string
/// literal sequence or a known string constant.
fn eval_value(toks: &[Tok], i: usize, constants: &BTreeMap<String, String>) -> Option<String> {
    let mut j = i;
    let mut s = String::new();
    let mut any = false;
    while let Some(Tok::Str(lit)) = toks.get(j) {
        s.push_str(lit);
        any = true;
        j += 1;
    }
    if any {
        return (toks.get(j) == Some(&Tok::Punct(';'))).then_some(s);
    }
    if let (Some(Tok::Ident(id)), Some(Tok::Punct(';'))) = (toks.get(i), toks.get(i + 1)) {
        return constants.get(id).cloned();
    }
    None
}

fn extract(src: &str, constants: &mut BTreeMap<String, String>, sources: &mut Vec<Source>) {
    let toks = tokenize(src);
    // Stack of open braces; `Some(index)` marks a ShaderSource struct body.
    let mut braces: Vec<Option<usize>> = Vec::new();
    let mut pending_struct: Option<String> = None;
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Ident(id) if id == "struct" && toks.get(i + 1) == Some(&Tok::Ident("ShaderSource".into())) => {
                // struct ShaderSource<BuiltIn::X, gfx::Backend::Type::OpenGL> {
                let mut j = i + 2;
                let mut built_in = None;
                let mut is_opengl = false;
                while j < toks.len() && toks[j] != Tok::Punct('{') && toks[j] != Tok::Punct(';') {
                    if toks[j] == Tok::Ident("BuiltIn".into())
                        && toks.get(j + 1) == Some(&Tok::Punct(':'))
                        && let Some(Tok::Ident(x)) = toks.get(j + 3)
                    {
                        built_in = Some(x.clone());
                    }
                    if toks[j] == Tok::Ident("OpenGL".into()) {
                        is_opengl = true;
                    }
                    j += 1;
                }
                if is_opengl && toks.get(j) == Some(&Tok::Punct('{')) {
                    pending_struct = built_in;
                }
                i = j;
                continue;
            }
            Tok::Punct('{') => {
                if let Some(built_in) = pending_struct.take() {
                    sources.push(Source { built_in, ..Default::default() });
                    braces.push(Some(sources.len() - 1));
                } else {
                    braces.push(None);
                }
            }
            Tok::Punct('}') => {
                braces.pop();
            }
            Tok::Punct('=') => {
                if let Some(Tok::Ident(lhs)) = i.checked_sub(1).and_then(|k| toks.get(k)) {
                    let current = braces.iter().rev().find_map(|b| *b);
                    let in_struct = braces.last().copied().flatten().is_some();
                    if let Some(value) = eval_value(&toks, i + 1, constants) {
                        if in_struct {
                            let s = &mut sources[current.unwrap()];
                            match lhs.as_str() {
                                "name" => s.name = Some(value),
                                "vertex" => s.vertex = Some(value),
                                "fragment" => s.fragment = Some(value),
                                _ => {}
                            }
                        } else if current.is_none() {
                            constants.insert(lhs.clone(), value);
                        }
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
}

fn upper_snake(camel: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = camel.chars().collect();
    for (k, &c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() && k > 0 {
            let prev = chars[k - 1];
            let next_lower = chars.get(k + 1).is_some_and(|n| n.is_ascii_lowercase());
            if prev.is_ascii_lowercase() || prev.is_ascii_digit() || (prev.is_ascii_uppercase() && next_lower) {
                out.push('_');
            }
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

fn raw_literal(s: &str) -> String {
    let mut hashes = 1;
    while s.contains(&format!("\"{}", "#".repeat(hashes))) {
        hashes += 1;
    }
    let h = "#".repeat(hashes);
    format!("r{h}\"{s}\"{h}")
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(args.len() >= 2, "usage: extract_glsl <out.rs> <header.hpp>...");
    let mut constants = BTreeMap::new();
    let mut sources = Vec::new();
    let mut headers: Vec<&String> = args[2..].iter().collect();
    headers.sort();
    for header in &headers {
        let text = std::fs::read_to_string(header).unwrap_or_else(|e| panic!("{header}: {e}"));
        extract(&text, &mut constants, &mut sources);
    }
    sources.sort_by(|a, b| a.built_in.cmp(&b.built_in));

    let mut out = String::new();
    out.push_str("// @generated by //crates/mln_gl/tools:extract_glsl from\n");
    for header in &headers {
        let file = std::path::Path::new(header.as_str()).file_name().unwrap().to_string_lossy();
        let _ = writeln!(out, "//   include/mln/shaders/gl/{file}");
    }
    out.push_str("// The GLSL is copied unchanged from the C++ raw string literals.\n\n");
    out.push_str("use super::shader_source::GlslShaderSource;\n\n");
    for s in &sources {
        let _ = writeln!(out, "/// `ShaderSource<BuiltIn::{}, gfx::Backend::Type::OpenGL>`", s.built_in);
        let _ = writeln!(out, "pub const {}: GlslShaderSource = GlslShaderSource {{", upper_snake(&s.built_in));
        let _ = writeln!(out, "    built_in: \"{}\",", s.built_in);
        let _ = writeln!(out, "    name: {},", raw_literal(s.name.as_deref().unwrap_or("")));
        let _ = writeln!(out, "    vertex: {},", raw_literal(s.vertex.as_deref().unwrap_or("")));
        let _ = writeln!(out, "    fragment: {},", raw_literal(s.fragment.as_deref().unwrap_or("")));
        out.push_str("};\n\n");
    }
    out.push_str("/// Every extracted specialisation, sorted by `BuiltIn` name.\n");
    out.push_str("pub const ALL: &[&GlslShaderSource] = &[\n");
    for s in &sources {
        let _ = writeln!(out, "    &{},", upper_snake(&s.built_in));
    }
    out.push_str("];\n");
    std::fs::write(&args[1], out).unwrap_or_else(|e| panic!("{}: {e}", args[1]));
}
