"""Summarize a Device Farm run's results as Markdown, one section per phone.

Usage: summary.py <results directory>

The directory holds what schedule.sh downloads: a directory per phone with
its customer artifacts (CUSTOMER_ARTIFACT.zip), whose junit.xml nextest wrote,
and its test spec output (TESTSPEC_OUTPUT.txt), whose end shows why a phone
ran no tests.
"""

from __future__ import annotations

import pathlib
import sys
import xml.etree.ElementTree as ET
import zipfile


def junit(phone: pathlib.Path) -> ET.Element | None:
    """The phone's JUnit report, if its tests ran."""
    artifacts = phone / "CUSTOMER_ARTIFACT.zip"
    if not artifacts.is_file():
        return None
    with zipfile.ZipFile(artifacts) as archive:
        for name in archive.namelist():
            if name.endswith("/junit.xml"):
                return ET.fromstring(archive.read(name))
    return None


def section(phone: pathlib.Path) -> str:
    lines = [f"### {phone.name.replace('-', ' ')}", ""]
    report = junit(phone)
    if report is None:
        output = phone / "TESTSPEC_OUTPUT.txt"
        tail = output.read_text(errors="replace").splitlines()[-40:] if output.is_file() else []
        return "\n".join([*lines, "The tests didn't run:", "", "```", *tail, "```", ""])
    tests = int(report.get("tests", 0))
    failed = [
        f"{case.get('classname')} {case.get('name')}"
        for case in report.iter("testcase")
        if case.find("failure") is not None or case.find("error") is not None
    ]
    lines.append(f"{tests - len(failed)} of {tests} tests passed.")
    if failed:
        lines += ["", "Failed:", "", "```", *failed, "```"]
    return "\n".join([*lines, ""])


def main() -> None:
    results = pathlib.Path(sys.argv[1])
    phones = sorted(phone for phone in results.iterdir() if phone.is_dir()) if results.is_dir() else []
    print("\n".join(section(phone) for phone in phones) if phones else "The run has no phone's results.")


if __name__ == "__main__":
    main()
