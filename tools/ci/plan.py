"""Select the jobs a CI run needs from the units that Rubberstamp planned."""

from __future__ import annotations

import json
import os
import pathlib

CATALOG = pathlib.Path(__file__).with_name("jobs.json")


def catalog() -> list[dict]:
    """Every job, with a variant only when it runs in multiple configurations.

    A job without a unit runs on every event.
    """
    return json.loads(CATALOG.read_text())


def known_units() -> set[str]:
    """The unit ids referenced by the job catalog."""
    return {row["unit"] for row in catalog() if "unit" in row}


def job_name(row: dict) -> str:
    return row["job"] + (f" ({row['variant']})" if "variant" in row else "")


def secrets_available(event_name: str, event: dict, repository: str) -> bool:
    """Whether the run may use repository secrets, such as deploy tokens."""
    if event_name == "push":
        return True
    if event_name == "pull_request":
        pr = event["pull_request"]
        head_repo = pr["head"].get("repo") or {}
        return (
            pr["user"]["login"] != "dependabot[bot]"
            and head_repo.get("full_name") == repository
        )
    if event_name == "workflow_dispatch":
        return str(event.get("inputs", {}).get("secrets", "true")).lower() != "false"
    return False


def parse_dispatch_units(raw: str) -> list[str] | None:
    """Parse an optional workflow_dispatch `units` JSON array input.

    Returns None when empty (run every unit), or validates and returns the list
    of unit ids.
    """
    text = raw.strip()
    if not text:
        return None
    try:
        parsed = json.loads(text)
    except json.JSONDecodeError as exc:
        raise SystemExit(f"error: inputs.units must be a JSON array: {exc}") from exc
    if not isinstance(parsed, list) or not all(
        isinstance(item, str) for item in parsed
    ):
        raise SystemExit("error: inputs.units must be a JSON array of unit names")
    unknown = sorted(set(parsed) - known_units())
    if unknown:
        raise SystemExit(
            f"error: unknown coverage units in inputs.units: {', '.join(unknown)}"
        )
    return parsed


def planned_units(
    event_name: str, event: dict, rubberstamp_all: str, rubberstamp_units: str
) -> list[str] | None:
    """Return the selected unit ids, or None to run every unit."""
    if event_name == "workflow_dispatch":
        raw = str((event.get("inputs") or {}).get("units") or "")
        return parse_dispatch_units(raw)
    if event_name == "pull_request" and rubberstamp_all == "false":
        return json.loads(rubberstamp_units)
    return None


def selected(units: list[str] | None) -> list[dict]:
    """The catalog rows for the planned units, or for every unit when None."""
    return [
        row
        for row in catalog()
        if "unit" not in row or units is None or row["unit"] in units
    ]


def outputs(rows: list[dict], secrets: bool) -> dict:
    """The plan job's outputs: the jobs that run and each matrix job's matrix."""
    matrices = {row["job"]: {"include": []} for row in catalog() if "variant" in row}
    for row in rows:
        if "variant" in row:
            matrices[row["job"]]["include"].append(
                {
                    key: value
                    for key, value in row.items()
                    if key not in ("job", "unit", "variant")
                }
            )
    return {
        "jobs": list(dict.fromkeys(row["job"] for row in rows)),
        "secrets": secrets,
        **matrices,
    }


def main() -> None:
    event_name = os.environ["GITHUB_EVENT_NAME"]
    event = json.loads(pathlib.Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
    units = planned_units(
        event_name,
        event,
        os.environ.get("RUBBERSTAMP_ALL", ""),
        os.environ.get("RUBBERSTAMP_UNITS", "[]"),
    )
    rows = selected(units)
    secrets = secrets_available(event_name, event, os.environ["GITHUB_REPOSITORY"])
    with pathlib.Path(os.environ["GITHUB_OUTPUT"]).open("a") as output:
        for key, value in outputs(rows, secrets).items():
            if isinstance(value, bool):
                encoded = str(value).lower()
            else:
                encoded = json.dumps(value, separators=(",", ":"))
            print(f"{key}={encoded}", file=output)
    jobs = ", ".join(job_name(row) for row in rows)
    summary_path = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary_path:
        with pathlib.Path(summary_path).open("a") as summary:
            if event_name == "workflow_dispatch" and units is not None:
                print(
                    f"\nManual dispatch units: `{json.dumps(units)}`",
                    file=summary,
                )
            print(f"\nJobs: {jobs}", file=summary)


if __name__ == "__main__":
    main()
