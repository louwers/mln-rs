#!/usr/bin/env bash
# Downloads a finished AWS Device Farm run's results (README.md): each phone's
# test spec output and artifacts into a directory of its own in the results
# directory, and prints a table of the phones' results. Exits 0 when the run
# passed.
#
# Usage: collect.sh <run ARN> <results directory>
set -euo pipefail

run=$1
results=$2
export AWS_REGION=us-west-2 AWS_PAGER=

mkdir -p "$results"
summary="| Phone | Result | Device minutes |"$'\n'"| --- | --- | --- |"
while IFS=$'\t' read -r job phone result minutes; do
    summary+=$'\n'"| $phone | $result | $minutes |"
    directory=$results/${phone// /-}
    mkdir -p "$directory"
    while IFS=$'\t' read -r type extension url; do
        curl --fail --silent --show-error --location --output "$directory/$type.$extension" "$url"
    done < <(aws devicefarm list-artifacts --arn "$job" --type FILE --output json |
        jq -r '.artifacts[] | select(.type == "TESTSPEC_OUTPUT" or .type == "CUSTOMER_ARTIFACT") | [.type, .extension, .url] | @tsv')
done < <(aws devicefarm list-jobs --arn "$run" --output json |
    jq -r '.jobs[] | [.arn, .device.name, .result, .deviceMinutes.total] | @tsv')

echo "$summary"
[[ $(aws devicefarm get-run --arn "$run" --query run.result --output text) == PASSED ]]
