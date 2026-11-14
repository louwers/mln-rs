//! The report as text for a terminal: aligned columns with the name last.

use std::fmt::Write as _;

use crate::{Sizes, changes, closing, largest, top_level_find};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Style {
    /// The terminal's width, which names are cut to.
    pub(crate) width: Option<usize>,
    /// Whether to color with ANSI escapes.
    pub(crate) color: bool,
}

const BAR_WIDTH: usize = 16;

pub(crate) fn report(title: &str, sizes: &Sizes, top: usize, style: Style) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}: {} without its name section", paint(title, Tone::Bold, style), kib(sizes.total));

    let groups = largest(&sizes.groups());
    let max = groups.first().map_or(1, |(_, bytes)| *bytes);
    let share = |bytes: u64| format!("{:.1}%", 100.0 * bytes as f64 / sizes.total as f64);
    let mut rows: Vec<Row> = groups
        .iter()
        .take(top)
        .map(|&(name, bytes)| Row::new([plain(kib(bytes)), plain(share(bytes)), plain(bar(bytes, max))], name))
        .collect();
    if groups.len() > top {
        let rest = groups[top..].iter().map(|(_, bytes)| bytes).sum();
        let name = format!("{} more", groups.len() - top);
        rows.push(Row::new([plain(kib(rest)), plain(share(rest)), plain(bar(0, max))], &name));
    }
    table(&mut out, "Crates", &[], &rows, style);

    let rows: Vec<Row> = largest(&sizes.functions())
        .into_iter()
        .take(top)
        .map(|(name, bytes)| Row::new([plain(format!("{} B", group(bytes)))], name))
        .collect();
    table(&mut out, "Largest functions", &[], &rows, style);
    out
}

pub(crate) fn compare(title: &str, base: &Sizes, sizes: &Sizes, top: usize, style: Style) -> String {
    let mut out = String::new();
    let (delta, percent) = change(base.total, sizes.total, kib, style);
    let _ = writeln!(
        out,
        "{}: {}, {} ({}) from {}",
        paint(title, Tone::Bold, style),
        kib(sizes.total),
        delta.painted,
        percent.painted,
        kib(base.total)
    );

    let rows: Vec<Row> = changes(&base.groups(), &sizes.groups())
        .into_iter()
        .take(top)
        .map(|(name, old, new)| {
            let (delta, percent) = change(old, new, kib, style);
            Row::new([plain(kib(old)), plain(kib(new)), delta, percent], name)
        })
        .collect();
    table(&mut out, "Crates", &["base", "now", "change", ""], &rows, style);

    let bytes = |n: u64| format!("{} B", group(n));
    let rows: Vec<Row> = changes(&base.functions(), &sizes.functions())
        .into_iter()
        .take(top)
        .map(|(name, old, new)| {
            let (delta, percent) = change(old, new, bytes, style);
            Row::new([plain(bytes(old)), plain(bytes(new)), delta, percent], name)
        })
        .collect();
    table(&mut out, "Functions", &["base", "now", "change", ""], &rows, style);
    out
}

/// A cell: its text, and the text to print, which may carry escapes.
struct Cell {
    text: String,
    painted: String,
}

fn plain(text: String) -> Cell {
    Cell { painted: text.clone(), text }
}

struct Row {
    cells: Vec<Cell>,
    name: String,
}

impl Row {
    fn new(cells: impl IntoIterator<Item = Cell>, name: &str) -> Self {
        Row { cells: cells.into_iter().collect(), name: name.to_owned() }
    }
}

/// Writes a titled table: right-aligned cells, then the name, cut to the
/// width that's left.
fn table(out: &mut String, title: &str, header: &[&str], rows: &[Row], style: Style) {
    let _ = writeln!(out, "\n{}", paint(title, Tone::Bold, style));
    if rows.is_empty() {
        let _ = writeln!(out, "{}", paint("  no change", Tone::Dim, style));
        return;
    }
    let columns = rows[0].cells.len();
    let widths: Vec<usize> = (0..columns)
        .map(|i| {
            let cells = rows.iter().map(|row| width(&row.cells[i].text));
            cells.chain(header.get(i).map(|h| width(h))).max().unwrap_or(0)
        })
        .collect();
    let used: usize = widths.iter().map(|w| w + 2).sum();
    if !header.is_empty() {
        let mut line = String::new();
        for (heading, width) in header.iter().zip(&widths) {
            let _ = write!(line, "{heading:>width$}  ");
        }
        let _ = writeln!(out, "{}", paint(line.trim_end(), Tone::Dim, style));
    }
    for row in rows {
        for (cell, column_width) in row.cells.iter().zip(&widths) {
            let pad = column_width - width(&cell.text);
            let _ = write!(out, "{:pad$}{}  ", "", cell.painted);
        }
        let name = match style.width {
            Some(total) => fit(&row.name, total.saturating_sub(used).max(24)),
            None => row.name.clone(),
        };
        let _ = writeln!(out, "{name}");
    }
}

fn width(text: &str) -> usize {
    text.chars().count()
}

/// The name in `max` characters: its deepest generic arguments collapsed to
/// `<…>` first, then cut at the end. The function's own generic arguments,
/// at the end, which tell its instances apart, keep a level more.
fn fit(name: &str, max: usize) -> String {
    if width(name) <= max {
        return name.to_owned();
    }
    for depth in (0..4).rev() {
        let collapsed = collapse(name, depth, depth + 1);
        if width(&collapsed) <= max {
            return collapsed;
        }
    }
    cut(&collapse(name, 0, 1), max)
}

/// The name with generic arguments nested deeper than `depth` as `<…>`, or
/// `last` for the arguments that end the name. A qualified path, `<T as
/// Trait>` or a leading `<T>`, isn't generic arguments, and is kept.
fn collapse(name: &str, depth: usize, last: usize) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    let mut at_start = true;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = closing(after) else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &after[..close];
        let limit = if after[close + 1..].is_empty() { last } else { depth };
        out.push('<');
        if (at_start && open == 0) || top_level_find(inner, " as ").is_some() {
            out.push_str(&collapse(inner, limit, limit));
        } else if limit == 0 {
            out.push('…');
        } else {
            out.push_str(&collapse(inner, limit - 1, limit - 1));
        }
        out.push('>');
        rest = &after[close + 1..];
        at_start = false;
    }
    out.push_str(rest);
    out
}

fn cut(text: &str, max: usize) -> String {
    if width(text) <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

#[derive(Debug, Clone, Copy)]
enum Tone {
    Bold,
    Dim,
    Grow,
    Shrink,
}

fn paint(text: &str, tone: Tone, style: Style) -> String {
    if !style.color {
        return text.to_owned();
    }
    let code = match tone {
        Tone::Bold => "1",
        Tone::Dim => "2",
        Tone::Grow => "31",
        Tone::Shrink => "32",
    };
    format!("\x1b[{code}m{text}\x1b[0m")
}

/// The change from `old` to `new` in `unit`, and in percent or as "new" or
/// "gone": red when it grew, green when it shrank.
fn change(old: u64, new: u64, unit: impl Fn(u64) -> String, style: Style) -> (Cell, Cell) {
    let (sign, tone) = if new >= old { ('+', Tone::Grow) } else { ('-', Tone::Shrink) };
    let delta = format!("{sign}{}", unit(new.abs_diff(old)));
    let percent = if old == 0 {
        "new".to_owned()
    } else if new == 0 {
        "gone".to_owned()
    } else if new >= 10 * old {
        format!("{:.0}×", new as f64 / old as f64)
    } else {
        format!("{sign}{:.1}%", 100.0 * new.abs_diff(old) as f64 / old as f64)
    };
    let cell = |text: String| Cell { painted: paint(&text, tone, style), text };
    (cell(delta), cell(percent))
}

/// A bar of `BAR_WIDTH` cells, full at `max`, in eighths of a cell.
fn bar(bytes: u64, max: u64) -> String {
    const EIGHTHS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let eighths = (bytes as f64 / max as f64 * (BAR_WIDTH * 8) as f64).round() as usize;
    let mut bar = "█".repeat(eighths / 8);
    if !eighths.is_multiple_of(8) {
        bar.push(EIGHTHS[eighths % 8]);
    }
    let pad = BAR_WIDTH - width(&bar);
    bar + &" ".repeat(pad)
}

fn kib(bytes: u64) -> String {
    let tenths = (bytes as f64 / 102.4).round() as u64;
    format!("{}.{} KiB", group(tenths / 10), tenths % 10)
}

/// The number with thousands separators.
fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::module;

    const PLAIN: Style = Style { width: None, color: false };

    #[test]
    fn reports_crates_and_functions() {
        let sizes = Sizes::parse(&module([10, 4], true)).unwrap();
        let report = report("m.wasm", &sizes, 2, PLAIN);
        assert!(report.contains("0.0 KiB") && report.contains("mln_util\n"), "{report}");
        assert!(report.lines().any(|line| line.split_whitespace().eq(["12", "B", "mln_util::f"])), "{report}");
        assert!(report.contains("more\n"), "the rows past --top add up: {report}");
    }

    #[test]
    fn compares_with_a_base() {
        let base = Sizes::parse(&module([10, 4], true)).unwrap();
        let sizes = Sizes::parse(&module([30, 4], true)).unwrap();
        let report = compare("m.wasm", &base, &sizes, 5, PLAIN);
        let row = ["12", "B", "32", "B", "+20", "B", "+166.7%", "mln_util::f"];
        assert!(report.lines().any(|line| line.split_whitespace().eq(row)), "{report}");
        assert!(!report.contains("drop_glue"), "unchanged functions are left out: {report}");
        let same = compare("m.wasm", &base, &base, 5, PLAIN);
        assert!(same.contains("no change"), "{same}");
        assert!(paint("x", Tone::Grow, Style { width: None, color: true }).starts_with("\x1b[31m"));
    }

    #[test]
    fn formats_numbers_and_bars() {
        assert_eq!(group(4_195_527), "4,195,527");
        assert_eq!(group(999), "999");
        assert_eq!(kib(4_195_527), "4,097.2 KiB");
        assert_eq!(bar(1, 1), "█".repeat(BAR_WIDTH));
        assert_eq!(bar(1, 64), "▎               ");
        assert_eq!(cut("abcdef", 4), "abc…");
    }

    #[test]
    fn collapses_generics_but_not_qualified_paths() {
        let name = "<&mut a::De<a::Slice> as b::De>::map::<<g::X<u8> as c::Des>::de::V>";
        assert_eq!(collapse(name, 1, 1), "<&mut a::De<a::Slice> as b::De>::map::<<g::X<…> as c::Des>::de::V>");
        assert_eq!(collapse(name, 0, 1), "<&mut a::De<…> as b::De>::map::<<g::X<…> as c::Des>::de::V>");
        assert_eq!(collapse(name, 0, 0), "<&mut a::De<…> as b::De>::map::<…>");
        assert_eq!(collapse("<[usize]>::sort_by::<F>", 0, 0), "<[usize]>::sort_by::<…>");
        assert_eq!(fit(name, 60), "<&mut a::De<…> as b::De>::map::<<g::X<…> as c::Des>::de::V>");
        assert_eq!(fit(name, 40), "<&mut a::De<…> as b::De>::map::<<g::X<……");
        assert_eq!(fit(name, 30), "<&mut a::De<…> as b::De>::map…");
    }
}
