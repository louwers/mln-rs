#!/usr/bin/env bash
# Schedules a test package that `mise run device-farm:package` built on AWS
# Device Farm's phones (README.md): reads the test spec, timeout, and phones
# from .config/device-farm.toml, uploads the package and test spec to the
# project that $AWS_DEVICE_FARM_PROJECT_ARN names (by default the one named
# mln-rs), schedules the run, and prints the run's ARN.
#
# Usage: schedule.sh <package.zip> <run name>
set -euo pipefail

package=$1
name=$2
repo_root=$(cd "$(dirname "$0")/../.." && pwd)
config=$repo_root/.config/device-farm.toml
export AWS_REGION=us-west-2 AWS_PAGER=

config_values=()
while IFS= read -r line; do
    config_values+=("$line")
done < <(
    python3 - "$config" << 'PYEOF'
import sys
import tomllib

with open(sys.argv[1], "rb") as f:
    cfg = tomllib.load(f)
print(cfg["test_spec_path"])
print(cfg.get("job_timeout_minutes", 60))
for arn in cfg["devices"]:
    print(arn)
PYEOF
)
spec=$repo_root/${config_values[0]}
job_minutes=${config_values[1]}
devices=("${config_values[@]:2}")

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
spec_arn_file=$(mktemp)
trap 'rm -f "$spec_arn_file"' EXIT
upload "$spec" APPIUM_WEB_NODE_TEST_SPEC > "$spec_arn_file" &
spec_upload=$!
package_arn=$(upload "$package" APPIUM_WEB_NODE_TEST_PACKAGE)
wait "$spec_upload"
spec_arn=$(< "$spec_arn_file")

# A web app test, the type of test that needs no app: android.yml drives the
# phone over adb.
selection=$(jq -cn '{filters: [{attribute: "ARN", operator: "IN", values: $ARGS.positional}], maxDevices: ($ARGS.positional | length)}' --args "${devices[@]}")
aws devicefarm schedule-run \
    --project-arn "$project" \
    --name "$name" \
    --device-selection-configuration "$selection" \
    --test "type=APPIUM_WEB_NODE,testPackageArn=$package_arn,testSpecArn=$spec_arn" \
    --execution-configuration "jobTimeoutMinutes=$job_minutes,videoCapture=false" \
    --query run.arn --output text
