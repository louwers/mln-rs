#!/usr/bin/env bash
# Runs a test package that `mise run device-farm:package` built on AWS Device
# Farm's phones (README.md): uploads it with android.yml to the project that
# $AWS_DEVICE_FARM_PROJECT_ARN names (by default the one named mln-rs),
# schedules it on the phones below, waits for the run and downloads each
# phone's test spec output and artifacts into the results directory. Exits 0
# when the run passed. Interrupted, it stops the run.
#
# Usage: schedule.sh <package.zip> <results directory> [run name]
#
# It only uploads the package, which CI's device-farm workflow takes from a
# pull request that may come from a fork: nothing here runs or unpacks it.
set -euo pipefail

package=$1
results=$2
name=${3:-mln-rs Android tests}
spec=$(dirname "$0")/android.yml
export AWS_REGION=us-west-2 AWS_PAGER=

# A Mali GPU and an Adreno one, on Android 16.
devices=(
    arn:aws:devicefarm:us-west-2::device:6D89F408709B4D3BBBBB17696A24B141 # Google Pixel 9
    arn:aws:devicefarm:us-west-2::device:092D78E7B4694568812D5FF0711CD346 # Samsung Galaxy S25
)
# Each phone's run of the tests, and the wait for the whole run, including
# the queue for the account's phones.
job_minutes=${DEVICE_FARM_JOB_MINUTES:-60}
wait_minutes=${DEVICE_FARM_WAIT_MINUTES:-210}

project=${AWS_DEVICE_FARM_PROJECT_ARN:-$(aws devicefarm list-projects --query "projects[?name=='mln-rs'].arn | [0]" --output text)}

# Uploads a file as the type and prints its ARN once Device Farm has
# processed it.
upload() {
    local response arn status
    response=$(aws devicefarm create-upload --project-arn "$project" --type "$2" --name "$(basename "$1")" --output json)
    arn=$(jq -r .upload.arn <<< "$response")
    curl --fail --silent --show-error --upload-file "$1" "$(jq -r .upload.url <<< "$response")" >&2
    for _ in {1..120}; do
        response=$(aws devicefarm get-upload --arn "$arn" --output json)
        status=$(jq -r .upload.status <<< "$response")
        case $status in
            SUCCEEDED)
                echo "$arn"
                return
                ;;
            FAILED)
                echo "Device Farm rejected $1: $(jq -r .upload.message <<< "$response")" >&2
                return 1
                ;;
        esac
        sleep 5
    done
    echo "Device Farm did not process $1 in 10 minutes" >&2
    return 1
}

echo "Uploading $package"
package_arn=$(upload "$package" APPIUM_WEB_NODE_TEST_PACKAGE)
spec_arn=$(upload "$spec" APPIUM_WEB_NODE_TEST_SPEC)

# A web app test, the type of test that needs no app: android.yml drives the
# phone over adb.
selection=$(jq -cn '{filters: [{attribute: "ARN", operator: "IN", values: $ARGS.positional}], maxDevices: ($ARGS.positional | length)}' --args "${devices[@]}")
run=$(aws devicefarm schedule-run \
    --project-arn "$project" \
    --name "$name" \
    --device-selection-configuration "$selection" \
    --test "type=APPIUM_WEB_NODE,testPackageArn=$package_arn,testSpecArn=$spec_arn" \
    --execution-configuration "jobTimeoutMinutes=$job_minutes,videoCapture=false" \
    --query run.arn --output text)
echo "Scheduled $run"

stop() {
    echo "Stopping $run"
    aws devicefarm stop-run --arn "$run" > /dev/null
    exit 1
}
trap stop INT TERM

deadline=$((SECONDS + wait_minutes * 60))
shown=
while true; do
    state=$(aws devicefarm get-run --arn "$run" --query '[run.status, run.result]' --output text) || state=$shown
    if [[ $state != "$shown" ]]; then
        echo "$(date -u +%H:%M:%S) ${state//$'\t'/ }"
        shown=$state
    fi
    [[ $state == COMPLETED* ]] && break
    if ((SECONDS > deadline)); then
        echo "The run did not finish in $wait_minutes minutes" >&2
        stop
    fi
    # In the background, so that a signal stops the run at once.
    sleep 30 &
    wait $!
done
trap - INT TERM

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
if [[ -n ${GITHUB_STEP_SUMMARY:-} ]]; then
    echo "$summary" >> "$GITHUB_STEP_SUMMARY"
fi
[[ $(aws devicefarm get-run --arn "$run" --query run.result --output text) == PASSED ]]
