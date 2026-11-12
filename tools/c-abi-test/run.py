#!/usr/bin/env python3
"""Run maplibre-native-ffi's ABI suite against crates/mln_c and compare the
failing cases with the backend's list: expected-failures.txt for Vulkan,
expected-failures-opengl.txt for OpenGL.

The suite binary is the one CMakeLists.txt builds. Each test file runs in a
process of its own, in parallel, as maplibre-native-ffi's CTest entries do.
A case that fails leaves its handles to the suite's tearDown, whose callbacks
can still reach the failed case's stack, so the file's later cases each run in
a process of their own. When a process dies (a crash, or the suite's hang
watchdog), the case it was running fails.

The run fails when a case fails that the list doesn't name, when a case that
the list names passes, or when the list names a case that the suite doesn't
have. `--update` rewrites the list with the cases that failed instead, so it
only changes where the review sees it.
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
# Each backend's list of the cases that fail, which differ by backend.
EXPECTED = {"vulkan": HERE / "expected-failures.txt", "opengl": HERE / "expected-failures-opengl.txt"}
FIXTURES = HERE / "testdata" / "tests" / "native" / "fixtures"
PREFIX = "tests/native/"
# Unity's result line: <file>:<line>:<case>:<PASS|FAIL|IGNORE>[:<message>].
RESULT = re.compile(r"^(?P<file>tests/native/[^:]+):\d+:(?P<case>\w+):(?P<outcome>PASS|FAIL|IGNORE)(?::(?P<message>.*))?")
# A process gets at least the watchdog's 30 s for each of its cases.
PROCESS_TIMEOUT_SECONDS = 600

# What `just c-abi-test` runs each backend's suite with.
COMMAND = {"vulkan": "just c-abi-test", "opengl": "MLN_C_TEST_BACKEND=opengl just c-abi-test"}

LIST_HEADER = """\
# The ABI suite's cases (tools/c-abi-test/testdata/tests/native, from
# maplibre-native-ffi) that fail against crates/mln_c under its {backend}
# preset, as <file>:<case>, sorted, each optionally followed by `  # ` and
# why it fails. This command fails when another case fails or a listed case
# passes:
#
#     {command}
#
# Implementing a function makes cases pass, and the command with `--update`
# then shrinks this list, keeping the reasons of the cases that stay.
"""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("binary", type=pathlib.Path, help="the suite binary (mln_native_abi_tests)")
    parser.add_argument("--backend", choices=EXPECTED, default="vulkan", help="the backend the binary was built for")
    parser.add_argument("--update", action="store_true", help="rewrite the backend's list with this run's failures")
    parser.add_argument("--jobs", type=int, default=os.cpu_count() or 1, help="test files to run at once")
    parser.add_argument("-v", "--verbose", action="store_true", help="print each failure's message")
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
    if args.verbose:
        for key in sorted(failures):
            print(f"  {key}: {failures[key]}")

    path, command = EXPECTED[args.backend], COMMAND[args.backend]
    expected = read_expected(path)
    if args.update:
        lines = (f"{key}  # {expected[key]}" if expected.get(key) else key for key in sorted(failures))
        header = LIST_HEADER.format(backend=args.backend, command=command)
        path.write_text(header + "".join(f"{line}\n" for line in lines))
        print(f"wrote {len(failures)} cases to {path.relative_to(HERE.parents[1])}")
        return 0

    unexpected = sorted(set(failures) - set(expected))
    passing = sorted(key for key in set(expected) & set(results) if key not in failures)
    stale = sorted(set(expected) - set(results))
    for key in unexpected:
        print(f"FAILED, not in the list: {key}: {failures[key]}")
    for key in passing:
        print(f"{results[key][0]}ED, but in the list (remove it): {key}")
    for key in stale:
        print(f"in the list, but the suite has no such case: {key}")
    if unexpected or passing or stale:
        print(f"the results differ from {path.name}; `{command} --update` rewrites it")
        return 1
    print(f"as {path.name} expects ({len(expected)} cases)")
    return 0


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


def read_expected(path: pathlib.Path) -> dict[str, str]:
    """The listed cases, each with its reason, or an empty one."""
    if not path.exists():
        return {}
    lines = (line.strip() for line in path.read_text().splitlines())
    entries = (line.partition("#") for line in lines if line and not line.startswith("#"))
    return {key.strip(): reason.strip() for key, _, reason in entries}


if __name__ == "__main__":
    sys.exit(main())
