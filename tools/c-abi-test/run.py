#!/usr/bin/env python3
"""Run maplibre-native-ffi's ABI suite against crates/mln_c.

The suite binary is the one CMakeLists.txt builds. Each test file runs in a
process of its own, in parallel, as maplibre-native-ffi's CTest entries do.
A case that fails leaves its handles to the suite's tearDown, whose callbacks
can still reach the failed case's stack, so the file's later cases each run in
a process of their own. When a process dies (a crash, or the suite's hang
watchdog), the case it was running fails.

The run fails when any case fails.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import os
import pathlib
import re
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
FIXTURES = HERE / "testdata" / "tests" / "native" / "fixtures"
PREFIX = "tests/native/"
# Unity's result line: <file>:<line>:<case>:<PASS|FAIL|IGNORE>[:<message>].
RESULT = re.compile(r"^(?P<file>tests/native/[^:]+):\d+:(?P<case>\w+):(?P<outcome>PASS|FAIL|IGNORE)(?::(?P<message>.*))?")
# A process gets at least the watchdog's 30 s for each of its cases.
PROCESS_TIMEOUT_SECONDS = 600


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("binary", type=pathlib.Path, help="the suite binary (mln_native_abi_tests)")
    parser.add_argument("--jobs", type=int, default=os.cpu_count() or 1, help="test files to run at once")
    args = parser.parse_args()

    env = dict(os.environ)
    env["MLN_FFI_TEST_FIXTURE_DIR"] = str(FIXTURES)
    binary = args.binary.resolve()

    cases = list_cases(binary, env)
    files: dict[str, list[str]] = {}
    for file, case in cases:
        files.setdefault(file, []).append(case)

    results: dict[str, tuple[str, str]] = {}
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for file_results in pool.map(lambda item: run_file(binary, env, *item), files.items()):
            results.update(file_results)

    failures = {key: message for key, (outcome, message) in results.items() if outcome == "FAIL"}
    ignored = sum(1 for outcome, _ in results.values() if outcome == "IGNORE")
    print(
        f"{len(results)} cases in {len(files)} files: {len(results) - len(failures) - ignored} passed, "
        f"{len(failures)} failed, {ignored} ignored"
    )
    for key in sorted(failures):
        print(f"FAILED: {key}: {failures[key]}")
    return 1 if failures else 0


def list_cases(binary: pathlib.Path, env: dict[str, str]) -> list[tuple[str, str]]:
    output = subprocess.run([binary, "-l"], env=env, check=True, capture_output=True, text=True).stdout
    cases = []
    for line in output.splitlines():
        file, _, case = line.partition(":")
        if file.startswith(PREFIX) and case:
            cases.append((file.removeprefix(PREFIX), case))
    if not cases:
        raise SystemExit(f"{binary} -l listed no cases")
    return cases


def run_file(binary: pathlib.Path, env: dict[str, str], file: str, cases: list[str]) -> dict[str, tuple[str, str]]:
    """Each case's outcome and message, keyed <file>:<case>."""
    together, died = run(binary, env, ["-f", PREFIX + file])
    results = {}
    later = []
    for index, case in enumerate(cases):
        key = f"{file}:{case}"
        # A case without a result is the one the process died in.
        results[key] = together.get(key, ("FAIL", died))
        if results[key][0] == "FAIL":
            later = cases[index + 1 :]
            break
    for case in later:
        alone, died = run(binary, env, ["-n", f"{PREFIX}{file}:{case}"])
        results[f"{file}:{case}"] = alone.get(f"{file}:{case}", ("FAIL", died))
    return results


def run(binary: pathlib.Path, env: dict[str, str], filter_args: list[str]) -> tuple[dict[str, tuple[str, str]], str]:
    """The outcomes the run reported, and why it stopped if it died."""
    try:
        process = subprocess.run(
            [binary, *filter_args],
            env=env,
            capture_output=True,
            text=True,
            errors="replace",
            timeout=PROCESS_TIMEOUT_SECONDS,
        )
        output, returncode = process.stdout + process.stderr, process.returncode
    except subprocess.TimeoutExpired as timeout:
        output, returncode = (timeout.stdout or b"").decode(errors="replace"), None
    results = {}
    for line in output.splitlines():
        match = RESULT.match(line)
        if match:
            key = f"{match['file'].removeprefix(PREFIX)}:{match['case']}"
            results[key] = (match["outcome"], (match["message"] or "").strip())
    hang = next((line for line in output.splitlines() if line.startswith("MLN_TEST_HANG")), None)
    if hang:
        died = "hung (the suite's watchdog)"
    elif returncode is None:
        died = f"timed out after {PROCESS_TIMEOUT_SECONDS} s"
    elif returncode < 0:
        died = f"died of signal {-returncode}"
    else:
        died = f"exited with status {returncode} before reporting"
    return results, died


if __name__ == "__main__":
    sys.exit(main())
