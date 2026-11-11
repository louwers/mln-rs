#!/usr/bin/env python3
"""Compares the WebGPU and Vulkan backends on an Android device (README.md):

    android.py <ndk> [--api <level>] [--rounds <n>] [--pause <seconds>] [--out <directory>] [--keep]
               [mln-bench run options...]

Builds mln-bench for aarch64-linux-android with the NDK in <ndk>, pushes it,
the basemap fixture and the render suite's cache database to the device adb
reaches (ANDROID_SERIAL picks one of several), and runs it there from the
shell, once per backend per round in ABBA order, so that the device warming
up hits both alike. Prints each metric's median, lowest and highest value
over the rounds, and leaves the reports and the last round's frames in the
output directory. Without a --filter it runs a few cases, a minute or two
per process.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import statistics
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TARGET = "aarch64-linux-android"
DEVICE = "/data/local/tmp/mln-bench"
BACKENDS = ("webgpu", "vulkan")
FILTERS = ("Still/", "Frame/Throughput", "Frame/Paced", "Load/Style/basemap")


def log(message: str) -> None:
    print(f"android: {message}", file=sys.stderr, flush=True)


def adb(*args: str, capture: bool = False) -> str:
    """Runs adb; its output is returned or goes to stderr, where the progress is."""
    stdout = subprocess.PIPE if capture else sys.stderr
    result = subprocess.run(["adb", *args], check=True, text=True, stdout=stdout)
    return result.stdout if capture else ""


def build(ndk: Path, api: int) -> Path:
    """Builds mln-bench with the NDK's clang as the C compiler and the linker."""
    toolchains = sorted((ndk / "toolchains/llvm/prebuilt").glob("*/bin"))
    if not toolchains:
        raise SystemExit(f"no toolchain in {ndk}/toolchains/llvm/prebuilt")
    clang = toolchains[0] / f"aarch64-linux-android{api}-clang"
    target = TARGET.replace("-", "_")
    env = dict(
        os.environ,
        **{
            f"CC_{target}": str(clang),
            f"AR_{target}": str(toolchains[0] / "llvm-ar"),
            f"CARGO_TARGET_{target.upper()}_LINKER": str(clang),
        },
    )
    command = ["cargo", "build", "--release", "-p", "mln-rs-bench", "--target", TARGET]
    subprocess.run(command, check=True, cwd=ROOT, env=env)
    return Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / TARGET / "release/mln-bench"


def push(binary: Path, fixtures: Path) -> None:
    adb("shell", f"rm -rf {DEVICE} && mkdir -p {DEVICE}")
    adb("push", str(binary), f"{DEVICE}/mln-bench")
    adb("push", str(fixtures), f"{DEVICE}/fixtures")
    cache = ROOT / "tools/render-test/testdata/metrics/cache-style.db"
    adb("push", str(cache), f"{DEVICE}/cache-style.db")
    adb("shell", f"chmod 755 {DEVICE}/mln-bench")


def battery_celsius() -> float:
    """The battery's temperature, the one the shell may read on every device."""
    match = re.search(r"temperature: (\d+)", adb("shell", "dumpsys", "battery", capture=True))
    return int(match.group(1)) / 10 if match else float("nan")


def run(backend: str, options: list[str]) -> dict:
    """One `mln-bench run` on the device; its report."""
    command = [
        f"MLN_RENDER_BACKEND={backend}",
        f"MLN_BENCH_CACHE_DB={DEVICE}/cache-style.db",
        f"TMPDIR={DEVICE}",
        *("./mln-bench", "run", "--json", "--fixtures", "fixtures"),
        *("--frames", f"frames-{backend}"),
        *options,
    ]
    script = f"cd {DEVICE} && " + " ".join(shlex.quote(word) for word in command)
    output = adb("shell", script, capture=True)
    report = json.loads(output.strip().splitlines()[-1])
    # The frontend renders with WebGPU when the Vulkan backend fails to start.
    gpu = report["environment"]["gpu"]
    if backend == "vulkan" and not gpu.endswith("(Vulkan)"):
        raise SystemExit(f"the Vulkan backend didn't start; the run rendered on {gpu}")
    return report


def table(reports: dict[str, list[dict]]) -> str:
    """Each metric's median and range per backend, and how the medians differ."""
    samples: dict[str, dict[tuple[str, str], list[float]]] = {}
    for backend, rounds in reports.items():
        for case in (case for report in rounds for case in report["cases"]):
            for metric in case["metrics"]:
                key = (case["name"], metric["name"])
                samples.setdefault(backend, {}).setdefault(key, []).append(metric["value"])
    lines = [
        f"{backend}: {rounds[-1]['environment']['gpu']}, {len(rounds)} rounds"
        for backend, rounds in reports.items()
    ]
    lines.append(
        f"{'case':34} {'metric':18} {'webgpu':>12} {'min - max':>25}"
        f" {'vulkan':>12} {'min - max':>25} {'change':>8}"
    )
    for (case, metric), webgpu in samples["webgpu"].items():
        vulkan = samples["vulkan"].get((case, metric))
        if not vulkan:
            continue
        a, b = statistics.median(webgpu), statistics.median(vulkan)
        change = f"{(b / a - 1) * 100:+7.1f}%" if a else ""
        lines.append(
            f"{case:34} {metric:18} {a:12.3f} {min(webgpu):12.3f} -{max(webgpu):12.3f}"
            f" {b:12.3f} {min(vulkan):12.3f} -{max(vulkan):12.3f} {change:>8}"
        )
    return "\n".join(lines)


def main() -> None:
    parser = argparse.ArgumentParser(usage=__doc__)
    parser.add_argument("ndk", type=Path)
    parser.add_argument("--api", type=int, default=30)
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--pause", type=float, default=0)
    parser.add_argument("--out", type=Path, default=ROOT / "target/bench-android")
    parser.add_argument("--keep", action="store_true")
    arguments, options = parser.parse_known_args()
    if "--filter" not in options:
        options += [word for name in FILTERS for word in ("--filter", name)]
    fixtures = ROOT / "target/bench-fixtures"
    if not (fixtures / "version").is_file():
        raise SystemExit(f"no basemap fixture in {fixtures}; prepare it with `just bench-fixtures`")

    push(build(arguments.ndk, arguments.api), fixtures)
    arguments.out.mkdir(parents=True, exist_ok=True)
    reports: dict[str, list[dict]] = {backend: [] for backend in BACKENDS}
    for number in range(arguments.rounds):
        for backend in BACKENDS if number % 2 == 0 else BACKENDS[::-1]:
            time.sleep(arguments.pause)
            celsius = battery_celsius()
            log(f"round {number + 1}/{arguments.rounds}: {backend}, the battery at {celsius} °C")
            report = run(backend, options)
            (arguments.out / f"{backend}-{number + 1}.json").write_text(json.dumps(report) + "\n")
            reports[backend].append(report)
    for backend in BACKENDS:
        adb("pull", f"{DEVICE}/frames-{backend}", str(arguments.out))
    if not arguments.keep:
        adb("shell", f"rm -rf {DEVICE}")
    print(table(reports))


if __name__ == "__main__":
    main()
