#!/usr/bin/env bash
# Schedules a test package that `mise run device-farm:package` built on AWS
# Device Farm's phones (README.md): uploads it with its platform's test spec to
# the project that $AWS_DEVICE_FARM_PROJECT_ARN names (by default the one named
# mln-rs), schedules it on the platform's phones below, and prints the run's
# ARN. The run takes the account's flat-rate devices, and waits while
# maplibre-native's runs have them all. With a key, it first stops the
# platform's runs that are scheduled with the same key and haven't finished,
# such as those of a pull request's earlier commit.
#
# Usage: schedule.sh <android|ios> <package directory> <run name> [key]
#
# Nothing here runs or unpacks the package, which may come from a fork
# (README.md).
set -euo pipefail

platform=$1
package=$2
name=$3
key=${4:-}
spec=$(dirname "$0")/$platform/test-spec.yml
export AWS_REGION=us-west-2 AWS_PAGER=

case $platform in
    android)
        # A Mali GPU and an Adreno one, on Android 16.
        devices=(
            arn:aws:devicefarm:us-west-2::device:6D89F408709B4D3BBBBB17696A24B141 # Google Pixel 9
            arn:aws:devicefarm:us-west-2::device:092D78E7B4694568812D5FF0711CD346 # Samsung Galaxy S25
        )
        # A web app test, the type of test that needs no app: the test spec
        # drives the phone over adb.
        type=APPIUM_WEB_NODE
        ;;
    ios)
        devices=(
            arn:aws:devicefarm:us-west-2::device:6200F380A4874FEB9C72EED72B863B67 # Apple iPhone 17 Pro, iOS 26
        )
        # An app test, whose app Device Farm signs and installs: the test spec
        # runs the app over devicectl.
        type=APPIUM_NODE
        ;;
    *)
        echo "No Device Farm tests for $platform: android or ios" >&2
        exit 1
        ;;
esac
# Each phone's run of the tests.
job_minutes=60

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

echo "Uploading $package" >&2
uploads=$(mktemp -d)
upload "$spec" "${type}_TEST_SPEC" > "$uploads/spec" &
spec_upload=$!
if [[ $platform == ios ]]; then
    upload "$package/app.ipa" IOS_APP > "$uploads/app" &
    app_upload=$!
fi
package_arn=$(upload "$package/tests.zip" "${type}_TEST_PACKAGE")
wait "$spec_upload"
spec_arn=$(< "$uploads/spec")
run=(--project-arn "$project")
if [[ $platform == ios ]]; then
    wait "$app_upload"
    run+=(--app-arn "$(< "$uploads/app")")
fi

name+=" $platform"
if [[ -n $key ]]; then
    name+=" key=$key"
    aws devicefarm list-runs --arn "$project" --output json |
        jq -r --arg suffix " $platform key=$key" '.runs[] | select(.status != "COMPLETED" and .status != "STOPPING" and (.name | endswith($suffix))) | .arn' |
        while read -r earlier; do
            echo "Stopping $earlier" >&2
            aws devicefarm stop-run --arn "$earlier" > /dev/null
        done
fi

selection=$(jq -cn '{filters: [{attribute: "ARN", operator: "IN", values: $ARGS.positional}], maxDevices: ($ARGS.positional | length)}' --args "${devices[@]}")
aws devicefarm schedule-run \
    "${run[@]}" \
    --name "$name" \
    --device-selection-configuration "$selection" \
    --test "type=$type,testPackageArn=$package_arn,testSpecArn=$spec_arn" \
    --execution-configuration "jobTimeoutMinutes=$job_minutes,videoCapture=false" \
    --configuration billingMethod=UNMETERED \
    --query run.arn --output text
