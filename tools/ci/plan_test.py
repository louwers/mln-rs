"""Exercise job selection from Rubberstamp units and the CI workflow's gates."""

from __future__ import annotations

import json
import os
import pathlib
import re
import subprocess
import tempfile
import unittest

from tools.ci.plan import (
    catalog,
    job_name,
    outputs,
    parse_dispatch_units,
    planned_units,
    secrets_available,
    selected,
)

ROOT = pathlib.Path(__file__).resolve().parents[2]
CI_WORKFLOW = ROOT / ".github/workflows/ci.yml"
RUBBERSTAMP = ROOT / ".config/rubberstamp.yml"
REPO = "maplibre/mln-rs"


def pr_event(*, author: str = "contributor", fork: bool = False) -> dict:
    return {
        "pull_request": {
            "user": {"login": author},
            "head": {"repo": {"full_name": "someone/fork" if fork else REPO}},
        },
    }


def configured_units() -> set[str]:
    """The unit ids in the Rubberstamp config's `ci.units` mapping."""
    section = RUBBERSTAMP.read_text().split("\nci:\n", 1)[1]
    units = section.split("\n  units:\n", 1)[1]
    return set(re.findall(r"^    ([a-z0-9][a-z0-9._-]*):$", units, re.MULTILINE))


def workflow_jobs() -> set[str]:
    """Top-level job keys declared in `.github/workflows/ci.yml`."""
    section = CI_WORKFLOW.read_text().split("\njobs:\n", 1)[1]
    return set(re.findall(r"^  ([a-z0-9][a-z0-9._-]*):$", section, re.MULTILINE))


def names(rows: list[dict]) -> set[str]:
    return {job_name(row) for row in rows}


def unit_names(*units: str) -> set[str]:
    return {job_name(row) for row in catalog() if row.get("unit", "") in units}


ALWAYS = unit_names("")


class UnitConfigTest(unittest.TestCase):
    def test_every_unit_owns_jobs_and_always_on_jobs_match(self) -> None:
        cataloged = {row["unit"] for row in catalog() if "unit" in row}
        self.assertEqual(cataloged, configured_units())
        self.assertEqual(ALWAYS, {"commits", "lint"})

    def test_rubberstamp_counts_passes_from_the_ci_workflow(self) -> None:
        self.assertIn(
            "\n  workflow: .github/workflows/ci.yml\n", RUBBERSTAMP.read_text()
        )

    def test_every_catalog_job_exists_in_ci_workflow_and_ci_passed_needs(self) -> None:
        catalog_jobs = {row["job"] for row in catalog()}
        defined = workflow_jobs()
        self.assertTrue(
            catalog_jobs.issubset(defined),
            f"missing workflow jobs: {sorted(catalog_jobs - defined)}",
        )
        ci_text = CI_WORKFLOW.read_text()
        ci_passed = ci_text.split("\n  ci-passed:\n", 1)[1]
        needs_match = re.search(r"^\s+needs:\s*\[([^\]]+)\]", ci_passed, re.MULTILINE)
        self.assertIsNotNone(needs_match, "ci-passed must declare needs")
        assert needs_match is not None
        # A list that wraps ends in a comma.
        needs = {item.strip() for item in needs_match.group(1).split(",") if item.strip()}
        self.assertEqual(needs, defined - {"ci-passed"})


class PlanTest(unittest.TestCase):
    def test_every_unit_runs_every_job(self) -> None:
        self.assertEqual(names(selected(None)), names(catalog()))

    def test_planned_units_run_only_their_jobs_and_always_on_jobs(self) -> None:
        for units in (
            ["macos"],
            ["ios"],
            ["linux-arm64"],
            ["windows-x64"],
            ["android"],
            ["freebsd"],
            ["ohos"],
            ["android-device-farm", "ios-device-farm"],
            ["web", "package"],
            ["linux-x64"],
        ):
            with self.subTest(units=units):
                self.assertEqual(names(selected(units)), ALWAYS | unit_names(*units))

    def test_no_planned_units_run_only_always_on_jobs(self) -> None:
        result = outputs(selected([]), secrets=True)
        self.assertEqual(result["jobs"], ["commits", "lint"])
        self.assertEqual(result["test"], {"include": []})
        self.assertEqual(result["backend"], {"include": []})
        self.assertEqual(result["target"], {"include": []})

    def test_device_farm_units_build_each_platforms_package(self) -> None:
        result = outputs(selected(["ios-device-farm"]), secrets=False)
        self.assertEqual(result["jobs"], ["commits", "lint", "prepare-device-tests"])
        self.assertEqual(
            result["prepare-device-tests"]["include"],
            [
                {
                    "platform": "ios",
                    "runner": "blacksmith-6vcpu-macos-26",
                    "mise_env": "target-apple",
                }
            ],
        )
        self.assertEqual(
            outputs(selected([]), secrets=False)["prepare-device-tests"],
            {"include": []},
        )

    def test_unknown_rubberstamp_units_select_nothing_extra(self) -> None:
        self.assertEqual(names(selected(["nonexistent"])), ALWAYS)

    def test_outputs_name_each_job_once_and_build_matrices(self) -> None:
        result = outputs(selected(["linux-x64", "linux-arm64", "web"]), secrets=False)
        self.assertEqual(
            result["jobs"], ["commits", "lint", "test", "backend", "web"]
        )
        self.assertFalse(result["secrets"])
        self.assertEqual(
            result["test"]["include"],
            [
                {
                    "os": "linux",
                    "runner": "blacksmith-4vcpu-ubuntu-2604",
                    "backend": "webgpu",
                    "coverage": "true",
                    "save_toolchains": "false",
                },
                {
                    "os": "linux-arm64",
                    "runner": "blacksmith-4vcpu-ubuntu-2604-arm",
                    "backend": "webgpu",
                    "coverage": "false",
                    "save_toolchains": "true",
                },
            ],
        )
        self.assertEqual(
            [
                (entry["os"], entry["backend"])
                for entry in result["backend"]["include"]
            ],
            [
                ("linux", "vulkan"),
                ("linux", "gl"),
                ("linux-arm64", "vulkan"),
                ("linux-arm64", "gl"),
            ],
        )
        self.assertEqual(result["target"], {"include": []})

    def test_android_unit_populates_target_matrix(self) -> None:
        result = outputs(selected(["android"]), secrets=False)
        self.assertEqual(
            result["jobs"], ["commits", "lint", "target"]
        )
        self.assertFalse(result["secrets"])
        self.assertEqual(result["test"], {"include": []})
        self.assertEqual(result["backend"], {"include": []})
        self.assertEqual(
            result["target"]["include"],
            [
                {
                    "target": "android",
                    "runner": "blacksmith-4vcpu-ubuntu-2604",
                    "mise_env": "target-android",
                    "device": "Android emulator",
                    "device_command": "bash tools/ci/boot-android.sh",
                    "clippy_targets": (
                        "aarch64-linux-android,armv7-linux-androideabi,"
                        "x86_64-linux-android,i686-linux-android"
                    ),
                    "test_target": "x86_64-linux-android",
                    "backend": "vulkan",
                }
            ],
        )

    def test_outputs_include_target_matrix_for_ios(self) -> None:
        result = outputs(selected(["ios"]), secrets=False)
        self.assertEqual(result["jobs"], ["commits", "lint", "target"])
        self.assertFalse(result["secrets"])
        self.assertEqual(result["test"], {"include": []})
        self.assertEqual(result["backend"], {"include": []})
        self.assertEqual(
            result["target"]["include"],
            [
                {
                    "target": "ios",
                    "runner": "blacksmith-6vcpu-macos-26",
                    "mise_env": "target-apple",
                    "device": "iOS simulator",
                    "device_command": "devices-cli get ios",
                    "clippy_targets": (
                        "aarch64-apple-ios,"
                        "aarch64-apple-ios-sim,"
                        "aarch64-apple-ios-macabi,"
                        "aarch64-apple-visionos,"
                        "aarch64-apple-visionos-sim"
                    ),
                    "test_target": "aarch64-apple-ios-sim",
                    "backend": "metal",
                }
            ],
        )
    def test_outputs_build_target_matrix_for_freebsd(self) -> None:
        result = outputs(selected(["freebsd"]), secrets=False)
        self.assertEqual(result["jobs"], ["commits", "lint", "target"])
        self.assertFalse(result["secrets"])
        self.assertEqual(result["test"], {"include": []})
        self.assertEqual(result["backend"], {"include": []})
        self.assertEqual(
            result["target"]["include"],
            [
                {
                    "target": "freebsd",
                    "runner": "blacksmith-4vcpu-ubuntu-2604",
                    "mise_env": "target-freebsd",
                    "device": "FreeBSD virtual machine",
                    "device_command": "bash tools/ci/boot-freebsd.sh",
                    "clippy_targets": "x86_64-unknown-freebsd",
                    "test_target": "x86_64-unknown-freebsd",
                    "backend": "vulkan",
                }
            ],
        )

    def test_ohos_unit_populates_target_matrix(self) -> None:
        result = outputs(selected(["ohos"]), secrets=False)
        self.assertEqual(result["jobs"], ["commits", "lint", "target"])
        self.assertFalse(result["secrets"])
        self.assertEqual(result["test"], {"include": []})
        self.assertEqual(result["backend"], {"include": []})
        self.assertEqual(
            result["target"]["include"],
            [
                {
                    "target": "ohos",
                    "runner": "blacksmith-4vcpu-ubuntu-2604",
                    "mise_env": "target-ohos",
                    "device": "OpenHarmony emulator",
                    "device_command": "devices-cli get ohos",
                    "clippy_targets": (
                        "aarch64-unknown-linux-ohos,"
                        "armv7-unknown-linux-ohos,"
                        "x86_64-unknown-linux-ohos"
                    ),
                    "test_target": "x86_64-unknown-linux-ohos",
                    "backend": "gl",
                }
            ],
        )
    def test_dispatch_units_override_and_validation(self) -> None:
        self.assertIsNone(parse_dispatch_units(""))
        self.assertIsNone(parse_dispatch_units("   "))
        self.assertEqual(parse_dispatch_units('["macos", "web"]'), ["macos", "web"])
        self.assertEqual(parse_dispatch_units("[]"), [])
        for bad in ("not-json", '{"unit": "macos"}', "[123]", '["nonexistent"]'):
            with self.subTest(bad=bad):
                with self.assertRaises(SystemExit):
                    parse_dispatch_units(bad)

    def test_planned_units_by_event(self) -> None:
        self.assertIsNone(planned_units("push", {}, "false", '["web"]'))
        self.assertIsNone(planned_units("pull_request", pr_event(), "true", '["web"]'))
        self.assertEqual(
            planned_units("pull_request", pr_event(), "false", '["web"]'),
            ["web"],
        )
        self.assertIsNone(
            planned_units("workflow_dispatch", {"inputs": {}}, "false", '["web"]')
        )
        self.assertEqual(
            planned_units(
                "workflow_dispatch",
                {"inputs": {"units": '["macos"]'}},
                "true",
                "[]",
            ),
            ["macos"],
        )

    def test_secrets_only_for_trusted_heads(self) -> None:
        def available(event_name: str, event: dict) -> bool:
            return secrets_available(event_name, event, REPO)

        self.assertTrue(available("push", {}))
        self.assertTrue(available("pull_request", pr_event()))
        self.assertFalse(available("pull_request", pr_event(fork=True)))
        self.assertFalse(available("pull_request", pr_event(author="dependabot[bot]")))
        deleted_fork = pr_event()
        deleted_fork["pull_request"]["head"]["repo"] = None
        self.assertFalse(available("pull_request", deleted_fork))
        self.assertTrue(available("workflow_dispatch", {}))
        for secrets in ("false", False):
            event = {"inputs": {"secrets": secrets}}
            self.assertFalse(available("workflow_dispatch", event))


class PlanCommandTest(unittest.TestCase):
    def run_plan(
        self,
        *,
        event_name: str,
        event_payload: dict,
        all_units: str = "true",
        units: str = "[]",
    ) -> tuple[dict[str, str], str]:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            event = root / "event.json"
            output = root / "output"
            summary = root / "summary"
            event.write_text(json.dumps(event_payload))
            subprocess.run(
                ["bash", str(ROOT / ".config/mise/tasks/ci/plan")],
                cwd=root,
                env={
                    "PATH": os.environ["PATH"],
                    "GITHUB_EVENT_NAME": event_name,
                    "GITHUB_EVENT_PATH": str(event),
                    "GITHUB_REPOSITORY": REPO,
                    "GITHUB_OUTPUT": str(output),
                    "GITHUB_STEP_SUMMARY": str(summary),
                    "RUBBERSTAMP_ALL": all_units,
                    "RUBBERSTAMP_UNITS": units,
                },
                check=True,
            )
            parsed = dict(
                line.split("=", 1) for line in output.read_text().splitlines()
            )
            return parsed, summary.read_text()

    def test_pull_request_subset(self) -> None:
        parsed, summary = self.run_plan(
            event_name="pull_request",
            event_payload=pr_event(),
            all_units="false",
            units='["web"]',
        )
        self.assertEqual(json.loads(parsed["jobs"]), ["commits", "lint", "web"])
        self.assertEqual(parsed["secrets"], "true")
        self.assertEqual(json.loads(parsed["test"]), {"include": []})
        self.assertEqual(json.loads(parsed["backend"]), {"include": []})
        self.assertEqual(json.loads(parsed["target"]), {"include": []})
        self.assertIn("Jobs: commits, lint, web", summary)

    def test_workflow_dispatch_units_override(self) -> None:
        parsed, summary = self.run_plan(
            event_name="workflow_dispatch",
            event_payload={"inputs": {"units": '["macos"]', "secrets": False}},
        )
        self.assertEqual(
            json.loads(parsed["jobs"]), ["commits", "lint", "test", "backend"]
        )
        self.assertEqual(parsed["secrets"], "false")
        self.assertEqual(
            [entry["os"] for entry in json.loads(parsed["test"])["include"]],
            ["macos"],
        )
        self.assertEqual(
            [entry["os"] for entry in json.loads(parsed["backend"])["include"]],
            ["macos"],
        )
        self.assertIn('Manual dispatch units: `["macos"]`', summary)


if __name__ == "__main__":
    unittest.main()
