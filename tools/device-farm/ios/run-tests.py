"""Run the iOS tests on the iPhone of an AWS Device Farm job (../README.md).

test-spec.yml runs this on the job's macOS test host, from the test package
that `mise run device-farm:package ios` builds. The phone has the package's
app, the test host (tools/ios-test-host), which holds the test binaries that
tests.json lists, each with its package and the package's directory in the
workspace.

Each binary runs in one launch of the app through devicectl, all its tests in
the app's process, as `cargo test` runs them: an app runs in one process at a
time, so a launch per test would take hours. The tests report to the harness's
JSON output. The tests that fail, or that a crash or a timeout left without a
result, get one more try in one more launch, and pass as flaky when they pass
then.

The job's artifacts get a JUnit report (junit.xml), each binary's log (logs/)
and the tests' output (test-output.tar.gz): the render suite's report, which
shows its failed tests' images, and the images that other tests write. Exits
1 when a test failed.
"""

from __future__ import annotations

import dataclasses
import json
import os
import pathlib
import re
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import xml.etree.ElementTree as ET

PACKAGE = pathlib.Path(os.environ["DEVICEFARM_TEST_PACKAGE_PATH"])
ARTIFACTS = pathlib.Path(os.environ["DEVICEFARM_LOG_DIR"])
DEVICE = os.environ["DEVICEFARM_DEVICE_UDID"]

# What the tests run with: the render suite on Metal with its API validation, as
# on the simulator (.config/mise/tasks/ci/target), and in the test's process,
# since an app can't start another. Stable Rust's harness writes JSON with
# RUSTC_BOOTSTRAP.
ENVIRONMENT = {
    "MLN_RENDER_BACKEND": "metal",
    "MLN_RENDER_TEST_JOBS": "1",
    "MTL_DEBUG_LAYER": "1",
    "MTL_DEBUG_LAYER_ERROR_MODE": "assert",
    "RUST_BACKTRACE": "1",
    "RUSTC_BOOTSTRAP": "1",
}
HARNESS = ["-Z", "unstable-options", "--format", "json", "--report-time"]

# The tests that nextest's target profile leaves out on the devices that run a
# copy of each package (.config/nextest.toml): the render suite through the
# worker codec, and the render runner's jobs, whose worker processes an app
# can't start. As the harness's --skip takes them: substrings of test names.
SKIP = {
    "mln-rs-render-test": ["render_tests::render_tests_through_the_codec", "jobs::"],
}

# A launch that runs longer is stopped, and its unfinished tests fail.
LAUNCH_TIMEOUT = 30 * 60


@dataclasses.dataclass
class Result:
    """A test's result: the harness's event, its time and its output."""

    event: str
    seconds: float = 0.0
    output: str = ""
    flaky: Result | None = None


@dataclasses.dataclass
class Launch:
    """What a launch of the app reported."""

    results: dict[str, Result]
    # The tests that started and didn't finish, as when the app crashed.
    unfinished: list[str]
    finished: bool


def devicectl(*arguments: str) -> None:
    """Run devicectl, and show what it printed if it fails."""
    process = subprocess.run(["xcrun", "devicectl", *arguments], capture_output=True, text=True, errors="replace")
    if process.returncode != 0:
        print(process.stdout, process.stderr, sep="", end="", file=sys.stderr)
        process.check_returncode()


def command(app: str, binary: dict, arguments: list[str]) -> list[str]:
    """devicectl's command that launches the app to run the binary's harness
    with the arguments, and gives the app's output as its own."""
    environment = {**ENVIRONMENT, "CARGO_MANIFEST_DIR": binary["dir"]}
    return [
        *("xcrun", "devicectl", "device", "process", "launch", "--device", DEVICE),
        *("--console", "--terminate-existing", "--environment-variables", json.dumps(environment)),
        *(app, binary["file"], *arguments),
    ]


def launch(app: str, binary: dict, arguments: list[str], log: pathlib.Path) -> Launch:
    """Run the binary's harness with the arguments in a launch of the app."""
    results: dict[str, Result] = {}
    started: list[str] = []
    finished = False
    with log.open("a") as output:
        output.write(f"$ {' '.join(arguments)}\n")
        output.flush()
        process = subprocess.Popen(
            command(app, binary, arguments), stdout=subprocess.PIPE, stderr=output, text=True, errors="replace"
        )
        # devicectl passes the signal on to the app.
        timer = threading.Timer(LAUNCH_TIMEOUT, process.terminate)
        timer.start()
        assert process.stdout is not None
        for line in process.stdout:
            event = parse(line)
            if event is None:
                output.write(line)
            elif event.get("type") == "suite" and event.get("event") in ("ok", "failed"):
                finished = True
            elif event.get("type") != "test":
                continue
            elif event.get("event") == "started":
                started.append(event["name"])
            # Not "timeout", which warns of a test that runs long.
            elif event.get("event") in ("ok", "failed", "ignored"):
                results[event["name"]] = Result(
                    event["event"], event.get("exec_time", 0.0), event.get("stdout", "")
                )
        process.wait()
        timer.cancel()
        if not finished:
            output.write(f"The harness stopped before it finished, or ran out of its {LAUNCH_TIMEOUT} seconds\n")
    return Launch(results, [name for name in started if name not in results], finished)


def parse(line: str) -> dict | None:
    """The harness's JSON event on a line of the app's output, if any."""
    if not line.startswith("{"):
        return None
    try:
        event = json.loads(line)
    except json.JSONDecodeError:
        return None
    return event if isinstance(event, dict) else None


def listed(app: str, binary: dict, log: pathlib.Path) -> list[str]:
    """The binary's tests that a run without filters runs, if it lists them."""
    arguments = [*skips(binary), "--list", "--format", "terse"]
    with log.open("a") as output:
        output.write(f"$ {' '.join(arguments)}\n")
        output.flush()
        try:
            stdout = subprocess.run(
                command(app, binary, arguments),
                stdout=subprocess.PIPE,
                stderr=output,
                text=True,
                errors="replace",
                timeout=300,
            ).stdout
        except subprocess.TimeoutExpired:
            return []
    return [match[1] for match in re.finditer(r"^(\S+): test$", stdout, re.MULTILINE)]


def skips(binary: dict) -> list[str]:
    return [argument for name in SKIP.get(binary["package"], []) for argument in ("--skip", name)]


def run(app: str, binary: dict, log: pathlib.Path) -> dict[str, Result]:
    """Run the binary's tests, and once more those that fail."""
    first = launch(app, binary, [*HARNESS, *skips(binary)], log)
    results = first.results
    unfinished = first.unfinished
    if not first.finished:
        # The tests that the crash or the timeout kept from starting.
        rest = [name for name in listed(app, binary, log) if name not in results and name not in unfinished]
        if rest:
            again = launch(app, binary, [*HARNESS, "--exact", *rest], log)
            results.update(again.results)
            unfinished += [name for name in rest if name not in again.results]
    crash = f"The app crashed or was stopped while the test ran: logs/{log.name} has its output."
    for name in unfinished:
        results[name] = Result("failed", output=crash)
    if not first.finished and not results:
        return {"(the binary)": Result("failed", output=f"The binary didn't run: logs/{log.name} has why.")}

    failed = [name for name, result in results.items() if result.event == "failed"]
    if failed:
        retry = launch(app, binary, [*HARNESS, "--exact", *failed], log)
        for name in failed:
            second = retry.results.get(name)
            if second and second.event == "ok":
                second.flaky = results[name]
                results[name] = second
    return results


def junit(binaries: list[tuple[dict, dict[str, Result]]]) -> ET.ElementTree:
    """A JUnit report of the results, in the shape of nextest's."""
    root = ET.Element("testsuites", name="mln-rs")
    total = failures = 0
    for binary, results in binaries:
        suite = ET.SubElement(root, "testsuite", name=binary["id"])
        ran = {name: result for name, result in results.items() if result.event != "ignored"}
        failed = sum(result.event != "ok" for result in ran.values())
        suite.set("tests", str(len(ran)))
        suite.set("failures", str(failed))
        total += len(ran)
        failures += failed
        for name, result in sorted(ran.items()):
            case = ET.SubElement(
                suite, "testcase", classname=binary["id"], name=name, time=f"{result.seconds:.3f}"
            )
            if result.flaky is not None:
                flaky = ET.SubElement(case, "flakyFailure", message="failed on its first try")
                ET.SubElement(flaky, "system-out").text = result.flaky.output
            elif result.event != "ok":
                ET.SubElement(case, "failure", message=f"test {result.event}").text = result.output
    root.set("tests", str(total))
    root.set("failures", str(failures))
    ET.indent(root)
    return ET.ElementTree(root)


def files(app: str, directory: str) -> list[dict]:
    """The files and directories in a directory of the app's data container."""
    with tempfile.NamedTemporaryFile() as listing:
        devicectl(
            *("device", "info", "files", "--device", DEVICE, "--no-recurse"),
            *("--domain-type", "appDataContainer", "--domain-identifier", app),
            *("--subdirectory", directory, "--json-output", listing.name),
        )
        return json.load(listing)["result"]["files"]


def collect_output(app: str) -> None:
    """Copy the tests' output from the app's data container to the artifacts.

    Of a render suite's output, whose top holds its HTML report, only the
    report, which shows its failed tests' images and query results: devicectl
    copies files one by one, and the suite writes thousands of images.
    """
    with tempfile.TemporaryDirectory() as work:
        output = pathlib.Path(work, "test-output")
        try:
            sources = []
            for entry in files(app, "Documents/test-output"):
                path = f"test-output/{entry['name']}"
                inside = files(app, f"Documents/{path}") if entry["resources"]["isDirectory"] else []
                reports = [f"{path}/{file['name']}" for file in inside if file["name"].endswith(".html")]
                sources += reports or [path]
            for source in sources:
                devicectl(
                    *("device", "copy", "from", "--device", DEVICE),
                    *("--domain-type", "appDataContainer", "--domain-identifier", app),
                    *("--source", f"Documents/{source}", "--destination", f"{work}/{source}"),
                )
        except subprocess.CalledProcessError:
            print("Couldn't collect the tests' output", file=sys.stderr)
            return
        with tarfile.open(ARTIFACTS / "test-output.tar.gz", "w:gz") as archive:
            archive.add(output, "test-output")


def main() -> None:
    tests = json.loads((PACKAGE / "tests.json").read_text())
    app = tests["app"]
    with tempfile.NamedTemporaryFile() as details:
        devicectl("device", "info", "details", "--device", DEVICE, "--json-output", details.name)
        result = json.load(details).get("result", {})
    hardware = result.get("hardwareProperties", {})
    software = result.get("deviceProperties", {})
    print(
        f"{hardware.get('marketingName')}, iOS {software.get('osVersionNumber')} ({software.get('osBuildUpdate')})",
        flush=True,
    )

    logs = ARTIFACTS / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    reports = []
    for binary in tests["binaries"]:
        log = logs / (re.sub(r"[^A-Za-z0-9_.-]", "_", binary["id"]) + ".log")
        start = time.monotonic()
        results = run(app, binary, log)
        reports.append((binary, results))
        failed = sorted(name for name, result in results.items() if result.event == "failed")
        flaky = sorted(name for name, result in results.items() if result.flaky is not None)
        passed = sum(result.event == "ok" for result in results.values())
        print(
            f"{binary['id']}: {passed} passed, {len(failed)} failed, {len(flaky)} flaky"
            f" in {time.monotonic() - start:.0f}s",
            flush=True,
        )
        for name in failed:
            print(f"    FAIL {name}", flush=True)
        for name in flaky:
            print(f"    FLAKY {name}", flush=True)

    junit(reports).write(ARTIFACTS / "junit.xml", encoding="utf-8", xml_declaration=True)
    collect_output(app)
    if any(result.event == "failed" for _, results in reports for result in results.values()):
        sys.exit(1)


if __name__ == "__main__":
    main()
