#!/usr/bin/env python3
"""Reports the verdicts of a comparison, for CI's bench workflows:

    verdicts.py <comparison.json> [--min-change <percent>] [--fail-on-worse]
    verdicts.py <comparison.json> [--min-change <percent>] --issue --head <sha> [--repo <owner/name>] [--dry-run]

Reads what `mln-bench compare --json` printed and prints its verdicts as
Markdown. `compare` gives a verdict to any change beyond a metric's noise,
however small; --min-change is this report's policy on which of them to act
on: smaller ones are listed apart, and neither fail the job nor touch the
issue. With --fail-on-worse it fails when a metric got worse. With --issue
it keeps the nightly issue instead: when a metric got worse, it opens the issue
titled TITLE with the verdicts, or comments on it when it is open already;
when none did, it closes an open one with a comment saying so. Through the gh
CLI, which the workflow logs in with its token; --dry-run prints what it would
do.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys

TITLE = "Nightly benchmarks: main got worse"


def number(value: float | None) -> str:
    """A metric's value, with enough digits to see the change."""
    if value is None:
        return "-"
    return f"{value:,.0f}" if abs(value) >= 1000 else f"{value:.4g}"


MIN_CHANGE = 0.0


def acted_on(metric: dict) -> bool:
    """Whether a metric's change is as large as --min-change asks."""
    change = metric["change"]
    return change is not None and abs(change) * 100 >= MIN_CHANGE


def verdict_rows(comparison: dict, verdict: str, large: bool = True) -> list[str]:
    """A Markdown table row for each metric that got `verdict`, of those
    --min-change acts on, or with `large` false, of the others."""
    rows = []
    for scenario in comparison["scenarios"]:
        for metric in scenario["metrics"]:
            if metric["verdict"] != verdict or acted_on(metric) != large:
                continue
            spread = metric["spread"]
            cells = [
                f"`{scenario['scenario']}`",
                metric["name"],
                number(metric["base"]),
                number(metric["candidate"]),
                f"{metric['change']:+.2%}",
                "" if spread is None else f"{spread:.2%}",
            ]
            rows.append("| " + " | ".join(cells) + " |")
    return rows


def environments(comparison: dict) -> list[str]:
    """Each environment the scenarios ran in, once, as a line."""
    lines = []
    for scenario in comparison["scenarios"]:
        environment = scenario["environment"]
        line = f"{environment['os']} {environment['arch']}, {environment['cpu']}"
        gpu = environment["gpu"]
        if gpu:
            line += f", {gpu['name']} ({gpu['backend']}{', CPU rasterizer' if gpu['software'] else ''})"
        if not environment["instruction_counter"]:
            line += ", no instruction counter"
        if line not in lines:
            lines.append(line)
    return lines


def markdown(comparison: dict) -> str:
    """The comparison's verdicts, and the scenarios that got none and why."""
    lines = [f"`mln-bench compare` against `{comparison['base_commit'][:12]}`: " + summary(comparison) + "."]
    lines += [f"- Ran on {environment}." for environment in environments(comparison)]
    for verdict, heading in (("worse", "Worse"), ("better", "Better")):
        rows = verdict_rows(comparison, verdict)
        if rows:
            lines += [
                "",
                f"### {heading}",
                "",
                "| scenario | metric | base | this | change | spread |",
                "|---|---|--:|--:|--:|--:|",
                *rows,
            ]
    small = verdict_rows(comparison, "worse", large=False) + verdict_rows(comparison, "better", large=False)
    if small:
        lines += [
            "",
            f"### Under {MIN_CHANGE:g}%",
            "",
            "| scenario | metric | base | this | change | spread |",
            "|---|---|--:|--:|--:|--:|",
            *small,
        ]
    skipped = [(s["scenario"], s["no_verdicts"]) for s in comparison["scenarios"] if s["no_verdicts"]]
    if skipped:
        lines += ["", "### No verdicts", "", *[f"- `{name}`: {reason}" for name, reason in skipped]]
    return "\n".join(lines) + "\n"


def summary(comparison: dict) -> str:
    worse = len(verdict_rows(comparison, "worse"))
    better = len(verdict_rows(comparison, "better"))
    over = f" by {MIN_CHANGE:g}% or more" if MIN_CHANGE else ""
    return f"{worse} worse, {better} better{over}, over {len(comparison['scenarios'])} scenarios"


def open_issue(repo: str | None) -> int | None:
    """The number of the open issue titled TITLE, if there is one."""
    args = ["gh", "issue", "list", "--state", "open", "--search", f'in:title "{TITLE}"', "--json", "number,title"]
    if repo:
        args += ["--repo", repo]
    issues = json.loads(subprocess.run(args, check=True, capture_output=True, text=True).stdout)
    return next((issue["number"] for issue in issues if issue["title"] == TITLE), None)


def gh(args: list[str], repo: str | None, dry_run: bool, text: str | None = None) -> None:
    args = ["gh", *args] + (["--repo", repo] if repo else [])
    if dry_run:
        print("would run:", " ".join(args) + (f" <<{len(text)} characters>>" if text else ""), file=sys.stderr)
        return
    subprocess.run(args, check=True, input=text, text=True)


def keep_issue(comparison: dict, head: str, repo: str | None, dry_run: bool) -> None:
    issue = open_issue(repo)
    if verdict_rows(comparison, "worse"):
        text = f"Main at {head} against main a day earlier.\n\n" + markdown(comparison)
        if issue is None:
            gh(["issue", "create", "--title", TITLE, "--body-file", "-"], repo, dry_run, text)
        else:
            gh(["issue", "comment", str(issue), "--body-file", "-"], repo, dry_run, text)
    elif issue is not None:
        text = f"Main at {head} against {comparison['base_commit']} gave no worse verdicts.\n"
        gh(["issue", "comment", str(issue), "--body-file", "-"], repo, dry_run, text)
        gh(["issue", "close", str(issue)], repo, dry_run)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("comparison")
    parser.add_argument("--min-change", type=float, default=0.0)
    parser.add_argument("--fail-on-worse", action="store_true")
    parser.add_argument("--issue", action="store_true")
    parser.add_argument("--head")
    parser.add_argument("--repo")
    parser.add_argument("--dry-run", action="store_true")
    options = parser.parse_args()
    if options.issue and not options.head:
        parser.error("--issue needs --head")
    global MIN_CHANGE
    MIN_CHANGE = options.min_change

    with open(options.comparison, encoding="utf-8") as file:
        comparison = json.load(file)
    print(markdown(comparison), end="")
    if options.issue:
        keep_issue(comparison, options.head, options.repo, options.dry_run)
    if options.fail_on_worse and verdict_rows(comparison, "worse"):
        print(f"verdicts: {summary(comparison)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
