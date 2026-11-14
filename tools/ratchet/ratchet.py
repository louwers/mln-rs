#!/usr/bin/env python3
"""Fail when a file has more library panic sites or `RefCell` borrows than recorded.

Two counts per Rust file, in the libraries' non-test code (`src/`,
`crates/*/src`, `vendor/*/src`, minus `#[cfg(test)]` items and the files of
`#[cfg(test)] mod name;` modules):

- `panic`: `.unwrap()`, `.expect(`, `panic!`, `unreachable!`, `todo!` and
  `unimplemented!`. A C++ exception or invariant ported as a panic takes down
  the embedding app: where C++ throws, return the error.
- `borrow`: `.borrow()` and `.borrow_mut()`, each a `RefCell` borrow that
  panics when it overlaps a conflicting one. The scan can't see types, so a
  file that names the `Borrow` or `BorrowMut` trait and never `RefCell` counts
  none: its calls are the trait's.

`tools/ratchet/baseline.toml` records the counts. A file absent from it allows
none, and a count above its baseline fails. A count below its baseline also
fails until `--update` lowers it, so the baseline only shrinks. `--update`
never raises a count or adds a file: a change that needs more raises the count
by hand, where the review sees it. When a file moves, rename its key by hand.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import subprocess
import sys
import tomllib

ROOT = pathlib.Path(__file__).resolve().parents[2]
BASELINE = ROOT / "tools" / "ratchet" / "baseline.toml"
RULES = ("panic", "borrow")

LIBRARY_SOURCE = re.compile(r"^(?:src/|(?:crates|vendor)/[^/]+/src/)")
PANIC = re.compile(
    r"\.unwrap\(\)|\.expect\(|\b(?:panic|unreachable|todo|unimplemented)!\s*[(\[{]"
)
BORROW = re.compile(r"\.borrow(?:_mut)?\(\)")
BORROW_TRAIT = re.compile(r"\bBorrow(?:Mut)?\b")
REF_CELL = re.compile(r"\bRefCell\b")
RUST_TEST_ATTRIBUTE = re.compile(r"#\[cfg\((?:all\()?test\b[^\]]*\]\s*")
PATH_ATTRIBUTE = re.compile(r'^#\[path\s*=\s*"([^"]+)"\]')
MOD_DECLARATION = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;")


def code_part(line: str) -> str:
    """The line without its `//` comment, skipping `//` inside string literals."""
    in_string = False
    index = 0
    while index < len(line):
        char = line[index]
        if in_string:
            if char == "\\":
                index += 1
            elif char == '"':
                in_string = False
        elif char == '"':
            in_string = True
        elif line.startswith("//", index):
            return line[:index]
        index += 1
    return line


def block_end(lines: list[str], start: int) -> int:
    """Index past the item at `start`, by counting braces from its first line."""
    depth = 0
    for index in range(start, len(lines)):
        line = code_part(lines[index])
        if depth == 0 and ";" in line and "{" not in line.split(";", 1)[0]:
            return index + 1
        depth += line.count("{") - line.count("}")
        if depth <= 0 and "{" in "".join(code_part(l) for l in lines[start : index + 1]):
            return index + 1
    return len(lines)


def test_lines(lines: list[str]) -> set[int]:
    """The indices of the lines of `#[cfg(test)]` items."""
    selected: set[int] = set()
    index = 0
    while index < len(lines):
        stripped = lines[index].strip()
        attribute = RUST_TEST_ATTRIBUTE.match(stripped)
        if not attribute:
            index += 1
            continue
        # The gated item follows the attribute on its line or below any
        # further attributes.
        start = index if stripped[attribute.end() :] else index + 1
        while start < len(lines) and lines[start].strip().startswith("#["):
            start += 1
        end = block_end(lines, start) if start < len(lines) else len(lines)
        selected.update(range(index, end))
        index = end
    return selected


def test_modules(path: str, lines: list[str]) -> list[str]:
    """Path prefixes of the files of the `#[cfg(test)] mod name;` modules."""
    directory, name = path.rsplit("/", 1)
    if name not in ("lib.rs", "main.rs", "mod.rs"):
        directory = f"{directory}/{name[: -len('.rs')]}"
    prefixes = []
    explicit = None
    for index in sorted(test_lines(lines)):
        stripped = lines[index].strip()
        attribute = PATH_ATTRIBUTE.match(stripped)
        if attribute:
            # Relative to the declaring file's directory.
            explicit = f"{path.rsplit('/', 1)[0]}/{attribute.group(1)}"
        declaration = MOD_DECLARATION.match(stripped)
        if declaration:
            module = declaration.group(1)
            if explicit:
                prefixes.append(explicit)
            else:
                prefixes += [f"{directory}/{module}.rs", f"{directory}/{module}/"]
            explicit = None
    return prefixes


def counts(path: str, lines: list[str], test_prefixes: list[str]) -> dict[str, int]:
    result = dict.fromkeys(RULES, 0)
    if not LIBRARY_SOURCE.match(path) or any(path.startswith(prefix) for prefix in test_prefixes):
        return result
    tests = test_lines(lines)
    code = [code_part(line) for index, line in enumerate(lines) if index not in tests]
    result["panic"] = sum(len(PANIC.findall(line)) for line in code)
    trait_only = any(BORROW_TRAIT.search(line) for line in code) and not any(
        REF_CELL.search(line) for line in code
    )
    if not trait_only:
        result["borrow"] = sum(len(BORROW.findall(line)) for line in code)
    return result


def scan() -> dict[str, dict[str, int]]:
    files = subprocess.run(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "*.rs"],
        cwd=ROOT,
        capture_output=True,
        check=True,
        text=True,
    ).stdout.split("\0")
    sources: dict[str, list[str]] = {}
    for path in files:
        if not path or "/testdata/" in path:
            continue
        try:
            sources[path] = (ROOT / path).read_text(encoding="utf-8").splitlines()
        except (FileNotFoundError, IsADirectoryError, UnicodeDecodeError):
            continue
    test_prefixes = [prefix for path, lines in sources.items() for prefix in test_modules(path, lines)]
    result: dict[str, dict[str, int]] = {rule: {} for rule in RULES}
    for path, lines in sources.items():
        for rule, count in counts(path, lines, test_prefixes).items():
            if count:
                result[rule][path] = count
    return result


def write_baseline(baseline: dict[str, dict[str, int]]) -> None:
    lines = [
        "# Library panic sites and RefCell borrows per file (tools/ratchet/ratchet.py).",
        "# Counts only fall: `mise run ratchet --update` lowers them. Raise one by hand",
        "# in the change that needs it.",
    ]
    for rule in RULES:
        lines.append("")
        lines.append(f"[{rule}]")
        for path, count in sorted(baseline.get(rule, {}).items()):
            lines.append(f'"{path}" = {count}')
    BASELINE.write_text("\n".join(lines) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--update", action="store_true", help="lower baseline counts to the current")
    args = parser.parse_args()
    current = scan()
    with BASELINE.open("rb") as file:
        baseline = tomllib.load(file)
    errors = []
    lowered: dict[str, dict[str, int]] = {}
    for rule in RULES:
        allowed = baseline.get(rule, {})
        found = current[rule]
        lowered[rule] = {
            path: min(count, found.get(path, 0)) for path, count in allowed.items() if found.get(path, 0)
        }
        for path in sorted(allowed.keys() | found.keys()):
            have, limit = found.get(path, 0), allowed.get(path, 0)
            if have > limit:
                errors.append(f"{path}: {have} {rule}, baseline {limit}")
            elif have < limit and not args.update:
                errors.append(f"{path}: {have} {rule}, baseline {limit}; run `mise run ratchet --update`")
    if args.update:
        write_baseline(lowered)
    for error in errors:
        print(f"error: {error}", file=sys.stderr)
    if errors:
        print(
            "Return errors instead of panicking in library code, and give state one "
            "owner instead of borrowing it from a RefCell (tools/ratchet/ratchet.py).",
            file=sys.stderr,
        )
    total = {rule: sum(current[rule].values()) for rule in RULES}
    print(", ".join(f"{rule}: {total[rule]}" for rule in RULES))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
