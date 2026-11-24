"""Retry a CI run once when only infrastructure steps failed."""

from __future__ import annotations

import json
import os
import subprocess

# Steps that provision the runner, check out code, install packages, or boot a
# device. A failure here says nothing about the code under test, so a single
# rerun is worth its cost. Every other step, build and test steps above all,
# fails the run for good.
INFRASTRUCTURE_STEPS = {
    "Complete job",
    "Check out repository",
    "Check out planner",
    "Plan coverage",
}
# `Set up ` covers the runner's own setup and setup actions, `Boot ` every
# emulator/simulator/VM boot step, `Install ` system/browser/lavapipe installs,
# `Post ` cleanup/cache-save hooks, and `Run <action>` unnamed checkout/cache/
# setup-ci-deps steps in ci.yml.
INFRASTRUCTURE_PREFIXES = (
    "Set up ",
    "Boot ",
    "Install ",
    "Post ",
    "Run useblacksmith/checkout@",
    "Run actions/checkout@",
    "Run actions/cache@",
    "Run ./.github/actions/setup-ci-deps",
)


def infrastructure_step(name: str) -> bool:
    return name in INFRASTRUCTURE_STEPS or name.startswith(INFRASTRUCTURE_PREFIXES)


def failed_in_infrastructure(job: dict) -> bool:
    failed = [
        step["name"]
        for step in job.get("steps", [])
        if step.get("conclusion") == "failure"
    ]
    # A job with no failed step timed out or lost its runner. A timeout can be
    # a hung test, so it does not count as infrastructure.
    return bool(failed) and all(infrastructure_step(name) for name in failed)


def retryable(jobs: list[dict]) -> bool:
    if any(job["conclusion"] not in {"success", "failure", "skipped"} for job in jobs):
        return False
    failed = [job for job in jobs if job["conclusion"] == "failure"]
    required = [job for job in failed if job["name"] == "ci-passed"]
    primary = [job for job in failed if job not in required]
    return (
        len(required) == 1
        and bool(primary)
        and all(failed_in_infrastructure(job) for job in primary)
    )


def retry_failed_run(run_id: str) -> None:
    path = f"repos/{os.environ['GITHUB_REPOSITORY']}/actions/runs/{run_id}"
    run = json.loads(subprocess.check_output(["gh", "api", path], text=True))
    if run["run_attempt"] != 1:
        print("This run already has a retry.")
        return
    pages = json.loads(
        subprocess.check_output(
            [
                "gh",
                "api",
                "--paginate",
                "--slurp",
                f"{path}/attempts/1/jobs?per_page=100",
            ],
            text=True,
        )
    )
    if not retryable([job for page in pages for job in page["jobs"]]):
        print("This run failed outside the infrastructure steps.")
        return
    subprocess.run(
        ["gh", "api", "--method", "POST", f"{path}/rerun-failed-jobs"], check=True
    )


def main() -> None:
    # A retry is an optimization; an API failure leaves the run for a person.
    retry_failed_run(os.environ["CI_RUN_ID"])


if __name__ == "__main__":
    main()
