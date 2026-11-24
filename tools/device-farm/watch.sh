#!/usr/bin/env bash
# Watches the AWS Device Farm runs that the device-test workflow scheduled
# (README.md): starts the device-farm-finish workflow for each run that
# finished, until none is in progress. A run's name starts with "mln-rs" and
# its commit. The AWS credentials last four hours, so after three it starts
# the next watcher and exits.
set -euo pipefail

export AWS_REGION=us-west-2 AWS_PAGER=
project=${AWS_DEVICE_FARM_PROJECT_ARN:?}

# The runs that a device-farm-finish run is named for.
finished=$(gh run list --workflow device-farm-finish.yml --limit 500 --json displayTitle --jq '.[].displayTitle')

while true; do
    if ((SECONDS > 3 * 3600)); then
        gh workflow run device-farm-watch.yml
        exit
    fi
    runs=$(aws devicefarm list-runs --arn "$project" --output json) || {
        sleep 30
        continue
    }
    in_progress=false
    while IFS=$'\t' read -r run status; do
        if [[ $status != COMPLETED ]]; then
            in_progress=true
        elif [[ $finished != *"$run"* ]]; then
            echo "Finishing $run"
            gh workflow run device-farm-finish.yml -f run="$run"
            finished+=" $run"
        fi
    done < <(jq -r --arg since "$(date -u -d '1 day ago' -Is)" '.runs[] |
        select((.name | test("^mln-rs [0-9a-f]{40} ")) and (.status != "COMPLETED" or .stopped > $since)) |
        [(.arn | split("/")[-1]), .status] | @tsv' <<< "$runs")
    $in_progress || exit 0
    sleep 30
done
