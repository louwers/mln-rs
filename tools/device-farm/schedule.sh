#!/usr/bin/env bash
# Schedules a test package that `mise run device-farm:package` built on AWS
# Device Farm's phones (README.md): reads the target's test type, test spec,
# timeout and phones from .config/device-farm.toml, uploads the package, its
# app if it has one, and the test spec to the project that
# $AWS_DEVICE_FARM_PROJECT_ARN names (by default the one named mln-rs),
# schedules the run, and prints the run's ARN.
#
# Usage: schedule.sh <target> <package directory> <run name>
#
# The target is a [targets.<target>] table of the config, such as android or
# ios. Nothing here runs or unpacks the package.
set -euo pipefail

target=$1
package=$2
name=$3
repo_root=$(cd "$(dirname "$0")/../.." && pwd)
export AWS_REGION=us-west-2 AWS_PAGER=

values=$(
    python3 - "$repo_root/.config/device-farm.toml" "$target" << 'PYEOF'
import sys
import tomllib

with open(sys.argv[1], "rb") as f:
    cfg = tomllib.load(f)
targets = cfg["targets"]
if sys.argv[2] not in targets:
    sys.exit(f"No [targets.{sys.argv[2]}] in {sys.argv[1]}: {', '.join(targets)}")
target = targets[sys.argv[2]]
print(cfg.get("billing_method", "METERED"))
print(target["test_type"])
print(target["test_spec_path"])
print(target["package_filename"])
print(target.get("app_filename", ""))
print(target.get("job_timeout_minutes", 60))
for arn in target["devices"]:
    print(arn)
PYEOF
)
config=()
while IFS= read -r line; do
    config+=("$line")
done <<< "$values"
billing=${config[0]}
type=${config[1]}
spec=$repo_root/${config[2]}
package_file=$package/${config[3]}
app_file=${config[4]:+$package/${config[4]}}
job_minutes=${config[5]}
devices=("${config[@]:6}")

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
trap 'rm -rf "$uploads"' EXIT
upload "$spec" "${type}_TEST_SPEC" > "$uploads/spec" &
spec_upload=$!
run=(--project-arn "$project")
if [[ -n $app_file ]]; then
    case $app_file in
        *.ipa) app_type=IOS_APP ;;
        *.apk) app_type=ANDROID_APP ;;
        *)
            echo "Not an app: $app_file" >&2
            exit 1
            ;;
    esac
    upload "$app_file" "$app_type" > "$uploads/app" &
    app_upload=$!
fi
package_arn=$(upload "$package_file" "${type}_TEST_PACKAGE")
wait "$spec_upload"
if [[ -n $app_file ]]; then
    wait "$app_upload"
    run+=(--app-arn "$(< "$uploads/app")")
fi

selection=$(jq -cn '{filters: [{attribute: "ARN", operator: "IN", values: $ARGS.positional}], maxDevices: ($ARGS.positional | length)}' --args "${devices[@]}")
aws devicefarm schedule-run \
    "${run[@]}" \
    --name "$name $target" \
    --device-selection-configuration "$selection" \
    --test "type=$type,testPackageArn=$package_arn,testSpecArn=$(< "$uploads/spec")" \
    --execution-configuration "jobTimeoutMinutes=$job_minutes,videoCapture=false" \
    --configuration "billingMethod=$billing" \
    --query run.arn --output text
