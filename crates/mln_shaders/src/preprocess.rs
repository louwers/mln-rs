//! The preprocessor of the embedded programs: it keeps the lines whose
//! `#if`, `#elif` and `#else` hold for a variant's flags, and puts each
//! macro's value in them.
//!
//! A directive is a line of its own, indented or not: `#if <condition>`,
//! `#elif <condition>`, `#else` or `#endif`. A condition is made of flags,
//! `true`, `false`, `!`, `&&`, `||` and parentheses, `!` binding tightest
//! and `||` loosest. A macro is an identifier that starts with `__`, which
//! WGSL reserves.

/// The lines of `text` that its directives keep with the flags `on` on,
/// each macro replaced by its value from `define`.
pub(crate) fn preprocess(
    text: &str,
    on: &dyn Fn(&str) -> bool,
    define: &dyn Fn(&str) -> Option<u32>,
) -> Result<String, String> {
    // For each open `#if`: whether its lines are kept, and whether one of
    // its branches has been.
    let mut open: Vec<(bool, bool)> = Vec::new();
    let mut kept = true;
    let mut out = String::with_capacity(text.len());
    for (number, line) in text.lines().enumerate() {
        let fail = |error: String| format!("line {}: {error}", number + 1);
        let trimmed = line.trim_start();
        let Some(directive) = trimmed.strip_prefix('#') else {
            if kept {
                expand(line, define, &mut out).map_err(fail)?;
                out.push('\n');
            }
            continue;
        };
        let (name, rest) = directive.split_once(char::is_whitespace).unwrap_or((directive, ""));
        match name {
            "if" => {
                let holds = all_kept(&open) && eval(rest, on).map_err(fail)?;
                open.push((holds, holds));
            }
            "elif" | "else" => {
                let &(_, taken) = open.last().ok_or_else(|| fail(format!("#{name} without #if")))?;
                open.pop();
                let holds = !taken && all_kept(&open) && (name == "else" || eval(rest, on).map_err(fail)?);
                open.push((holds, taken || holds));
            }
            "endif" => {
                open.pop().ok_or_else(|| fail("#endif without #if".to_owned()))?;
            }
            _ => return Err(fail(format!("unknown directive #{name}"))),
        }
        kept = all_kept(&open);
    }
    if !open.is_empty() {
        return Err("#if without #endif".to_owned());
    }
    Ok(out)
}

/// Whether every open `#if` keeps its lines.
fn all_kept(open: &[(bool, bool)]) -> bool {
    open.iter().all(|&(kept, _)| kept)
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Appends `line` to `out` with its macros expanded.
fn expand(line: &str, define: &dyn Fn(&str) -> Option<u32>, out: &mut String) -> Result<(), String> {
    let mut rest = line;
    while let Some(at) = rest.find("__") {
        // A `__` inside an identifier starts no macro.
        let start = rest[..at].rfind(|c| !is_ident(c)).map_or(0, |i| i + 1);
        let end = at + rest[at..].find(|c| !is_ident(c)).unwrap_or(rest.len() - at);
        if start < at {
            out.push_str(&rest[..end]);
        } else {
            let name = &rest[at..end];
            let value = define(name).ok_or_else(|| format!("undefined macro {name}"))?;
            out.push_str(&rest[..at]);
            out.push_str(&value.to_string());
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    Ok(())
}

/// The value of the condition `expression` with the flags `on` on.
pub(crate) fn eval(expression: &str, on: &dyn Fn(&str) -> bool) -> Result<bool, String> {
    let mut parser = Parser { rest: expression, on };
    let value = parser.or()?;
    parser.skip_space();
    if !parser.rest.is_empty() {
        return Err(format!("unexpected {:?} in {expression:?}", parser.rest));
    }
    Ok(value)
}

struct Parser<'a> {
    rest: &'a str,
    on: &'a dyn Fn(&str) -> bool,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        self.rest = self.rest.trim_start();
    }

    /// Takes `token` if it is next.
    fn eat(&mut self, token: &str) -> bool {
        self.skip_space();
        self.rest.strip_prefix(token).map(|rest| self.rest = rest).is_some()
    }

    fn or(&mut self) -> Result<bool, String> {
        let mut value = self.and()?;
        while self.eat("||") {
            value |= self.and()?;
        }
        Ok(value)
    }

    fn and(&mut self) -> Result<bool, String> {
        let mut value = self.not()?;
        while self.eat("&&") {
            value &= self.not()?;
        }
        Ok(value)
    }

    fn not(&mut self) -> Result<bool, String> {
        if self.eat("!") {
            return Ok(!self.not()?);
        }
        if self.eat("(") {
            let value = self.or()?;
            return if self.eat(")") { Ok(value) } else { Err("a ( without its )".to_owned()) };
        }
        self.skip_space();
        let end = self.rest.find(|c| !is_ident(c)).unwrap_or(self.rest.len());
        let (word, rest) = self.rest.split_at(end);
        self.rest = rest;
        match word {
            "" => Err("a condition expected".to_owned()),
            "true" => Ok(true),
            "false" => Ok(false),
            flag => Ok((self.on)(flag)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags<'a>(on: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |flag| on.contains(&flag)
    }

    #[test]
    fn evaluates_conditions() {
        let on = flags(&["A", "C"]);
        for (expression, expected) in [
            ("A", true),
            ("B", false),
            ("!B", true),
            ("A && B", false),
            ("A || B", true),
            ("!A || C && !B", true),
            ("!(A || B)", false),
            ("(A && (B || C)) && !!C", true),
            ("true && !false", true),
            ("B || false", false),
            ("A && B || C", true),
            ("!A && B || !C", false),
        ] {
            assert_eq!(eval(expression, &on), Ok(expected), "{expression}");
        }
        for wrong in ["", "A &&", "(A", "A)", "A & B", "A B", "!"] {
            assert!(eval(wrong, &on).is_err(), "{wrong}");
        }
    }

    #[test]
    fn keeps_the_lines_of_the_branches_that_hold() {
        let text = "a\n#if A\nb\n    #if B\n    c\n    #elif C\n    d\n    #else\n    e\n    #endif\n#else\nf\n#endif\n#if B\ng\n#elif !B\nh\n#elif A\ni\n#endif\nj\n";
        let none = |_: &str| None;
        assert_eq!(preprocess(text, &flags(&["A", "C"]), &none).unwrap(), "a\nb\n    d\nh\nj\n");
        assert_eq!(preprocess(text, &flags(&["A", "B"]), &none).unwrap(), "a\nb\n    c\ng\nj\n");
        assert_eq!(preprocess(text, &flags(&["A"]), &none).unwrap(), "a\nb\n    e\nh\nj\n");
        assert_eq!(preprocess(text, &flags(&[]), &none).unwrap(), "a\nf\nh\nj\n");
    }

    #[test]
    fn rejects_unbalanced_directives() {
        let on = flags(&[]);
        let none = |_: &str| None;
        for wrong in ["#if A\n", "#endif\n", "#else\n", "#if A\n#endif\n#elif B\n", "#ifdef A\n#endif\n"] {
            assert!(preprocess(wrong, &on, &none).is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn expands_macros() {
        let define = |name: &str| match name {
            "__g" => Some(2),
            "__b_1" => Some(17),
            _ => None,
        };
        let on = flags(&[]);
        let text = "@group(__g) @binding(__b_1) var a__b: f32;\n";
        assert_eq!(preprocess(text, &on, &define).unwrap(), "@group(2) @binding(17) var a__b: f32;\n");
        assert!(preprocess("@group(__other)\n", &on, &define).is_err());
        // A macro in a line that isn't kept needs no value.
        assert_eq!(preprocess("#if A\n__other\n#endif\n", &on, &define).unwrap(), "");
    }
}
