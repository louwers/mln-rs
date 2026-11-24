"""Unit tests for tools/ci/retry.py."""

from __future__ import annotations

import unittest

from tools.ci.retry import failed_in_infrastructure, infrastructure_step, retryable


def make_job(name: str, *failed_steps: str) -> dict:
    return {
        "name": name,
        "conclusion": "failure",
        "steps": [
            {"name": "Set up job", "conclusion": "success"},
            *(
                {"name": step, "conclusion": "failure"}
                for step in failed_steps
            ),
        ],
    }


class RetryTest(unittest.TestCase):
    def test_infrastructure_steps_are_recognized(self):
        for step in (
            "Set up job",
            "Complete job",
            "Check out repository",
            "Check out planner",
            "Plan coverage",
            "Boot iOS simulator",
            "Boot Android emulator",
            "Boot OpenHarmony emulator",
            "Boot FreeBSD virtual machine",
            "Install system packages",
            "Install lavapipe",
            "Install Chromium",
            "Install a CJK font",
            "Post Run useblacksmith/checkout@75c49b6ed30ce59336e9252425796cfcfac155f8",
            "Run useblacksmith/checkout@75c49b6ed30ce59336e9252425796cfcfac155f8",
            "Run actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1",
            "Run actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9",
            "Run ./.github/actions/setup-ci-deps",
        ):
            with self.subTest(step=step):
                self.assertTrue(infrastructure_step(step))

    def test_code_and_test_steps_are_not_infrastructure(self):
        for step in (
            "Select jobs",
            "Lint",
            "Package",
            "Clippy",
            "Test",
            "Test the C ABI",
            "Clippy and test",
            "Check",
            "Check the WebGL2 module",
            "Check the C library's packages",
            "Typecheck the page",
            "Test the page's wheel reading",
            "Deploy to Cloudflare Pages",
            "Require plan and every planned job",
        ):
            with self.subTest(step=step):
                self.assertFalse(infrastructure_step(step))

    def test_only_infrastructure_step_failures_retry(self):
        aggregates = [
            {"name": "ci-passed", "conclusion": "failure"},
            {"name": "web-preview", "conclusion": "skipped"},
        ]
        target = "target (android)"
        for failed in (
            ["Set up job"],
            ["Run ./.github/actions/setup-ci-deps"],
            ["Boot Android emulator"],
            ["Boot FreeBSD virtual machine"],
            ["Install lavapipe"],
            ["Post Run ./.github/actions/setup-ci-deps"],
        ):
            with self.subTest(failed=failed):
                self.assertTrue(retryable([make_job(target, *failed), *aggregates]))

        suite = "Clippy and test"
        for jobs in (
            [make_job(target, suite)],
            [make_job(target, "Boot Android emulator", suite)],
            # A job that failed without a failed step timed out, possibly in a
            # hung test.
            [make_job(target)],
            [make_job(target, "Boot Android emulator"), make_job("lint", "Lint")],
            [make_job("plan", "Select jobs")],
        ):
            with self.subTest(jobs=jobs):
                self.assertFalse(retryable([*jobs, *aggregates]))

    def test_multiple_infra_failures_and_plan_coverage_retry(self):
        aggregates = [{"name": "ci-passed", "conclusion": "failure"}]
        self.assertTrue(
            retryable(
                [
                    make_job("target (android)", "Boot Android emulator"),
                    make_job("backend (linux-x64, vulkan)", "Install lavapipe"),
                    *aggregates,
                ]
            )
        )
        self.assertTrue(
            retryable(
                [
                    make_job("plan", "Plan coverage"),
                    make_job("ci-passed", "Require plan and every planned job"),
                ]
            )
        )

    def test_non_terminal_or_missing_aggregate_do_not_retry(self):
        aggregates = [{"name": "ci-passed", "conclusion": "failure"}]
        self.assertFalse(retryable(aggregates))
        self.assertFalse(retryable([make_job("target (ios)", "Boot iOS simulator")]))
        self.assertFalse(
            retryable(
                [
                    make_job("target (ios)", "Boot iOS simulator"),
                    *aggregates,
                    {"name": "other", "conclusion": "cancelled"},
                ]
            )
        )
        self.assertFalse(failed_in_infrastructure({"name": "lint", "steps": []}))


if __name__ == "__main__":
    unittest.main()
