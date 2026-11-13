use std::collections::HashMap;

fn is_space(b: u8) -> bool {
    // std::isspace in the "C" locale
    matches!(b, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

#[must_use]
pub fn is_directive(line: &str, directive: &str) -> bool {
    let b = line.as_bytes();
    let mut pos = 0;
    while pos < b.len() && is_space(b[pos]) {
        pos += 1;
    }
    if pos >= b.len() || b[pos] != b'#' {
        return false;
    }
    pos += 1;
    while pos < b.len() && is_space(b[pos]) {
        pos += 1;
    }
    if !line[pos..].starts_with(directive) {
        return false;
    }
    pos += directive.len();
    pos == b.len() || is_space(b[pos])
}

#[must_use]
pub fn get_directive_argument(line: &str) -> String {
    let Some(hash) = line.find('#') else {
        return String::new();
    };
    let is_blank = |c: char| c == ' ' || c == '\t';
    // find_first_not_of(" \t", hash + 1), then find_first_of(" \t", pos)
    let Some(pos) = line[hash + 1..].find(|c| !is_blank(c)).map(|p| p + hash + 1) else {
        return String::new();
    };
    let Some(pos) = line[pos..].find(is_blank).map(|p| p + pos) else {
        return String::new();
    };
    let Some(pos) = line[pos..].find(|c| !is_blank(c)).map(|p| p + pos) else {
        return String::new();
    };
    let end = line[pos..].find([' ', '\t', '\r', '\n']).map(|p| p + pos);
    match end {
        Some(end) => line[pos..end].to_owned(),
        None => line[pos..].to_owned(),
    }
}

struct ConditionalFrame {
    parent_active: bool,
    condition: bool,
    else_seen: bool,
}

/// Strips `#ifdef`/`#ifndef`/`#else`/`#endif` blocks according to
/// `defines` (only key presence matters).
#[must_use]
pub fn preprocess_wgsl(source: &str, defines: &HashMap<String, bool>) -> String {
    let mut output = String::with_capacity(source.len());
    let mut stack: Vec<ConditionalFrame> = Vec::new();
    let mut current_active = true;

    // std::getline: split on '\n'; the text after the last '\n' is a line
    // only if it is non-empty (so "" and "a\n" have no trailing empty line).
    let mut lines: Vec<&str> = source.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    for line in lines {
        if is_directive(line, "ifdef") {
            let condition = defines.contains_key(&get_directive_argument(line));
            stack.push(ConditionalFrame { parent_active: current_active, condition, else_seen: false });
            current_active = current_active && condition;
            continue;
        }
        if is_directive(line, "ifndef") {
            let condition = !defines.contains_key(&get_directive_argument(line));
            stack.push(ConditionalFrame { parent_active: current_active, condition, else_seen: false });
            current_active = current_active && condition;
            continue;
        }
        if is_directive(line, "else") {
            let Some(frame) = stack.last_mut() else {
                continue;
            };
            if frame.else_seen {
                current_active = false;
            } else {
                frame.else_seen = true;
                current_active = frame.parent_active && !frame.condition;
            }
            continue;
        }
        if is_directive(line, "endif") {
            let Some(frame) = stack.pop() else {
                continue;
            };
            current_active = frame.parent_active;
            continue;
        }
        if current_active {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditionals() {
        let mut defines = HashMap::new();
        defines.insert("A".to_owned(), true);
        let src = "x\n#ifdef A\na\n#else\nnot_a\n#endif\n#ifndef B\nno_b\n#endif\n  #  ifdef  B  \nb\n#endif\ny";
        assert_eq!(preprocess_wgsl(src, &defines), "x\na\nno_b\ny\n");
        assert_eq!(get_directive_argument("#ifdef   HAS_UNIFORM_u_color  "), "HAS_UNIFORM_u_color");
        assert!(!is_directive("#ifdefX", "ifdef"));
    }
}
