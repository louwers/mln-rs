#!/usr/bin/env bash
# Runs the Android tests on the phone of an AWS Device Farm job: test-spec.yml
# runs it on the job's test host, from the test package that
# `mise run device-farm:package android` builds (../README.md). cargo-nextest
# runs the package's archive of the tests with its device-farm profile, each
# test on the phone in a Java VM's process through
# tools/android-test-host/runner.sh and devices-cli, on Vulkan. The job's
# artifacts get nextest's JUnit report and the tests' output
# (test-output.tar.gz): the render suite's report, which shows its failed
# tests' images, and the images that other tests write. Exits as nextest does.
set -euo pipefail

package=${DEVICEFARM_TEST_PACKAGE_PATH:?}
artifacts=${DEVICEFARM_LOG_DIR:?}
export ANDROID_SERIAL=${DEVICEFARM_DEVICE_UDID:?}
export PATH=$package/bin:$PATH

property() {
    adb shell getprop "$1" | tr -d '\r'
}
api=$(property ro.build.version.sdk)
echo "$(property ro.product.manufacturer) $(property ro.product.model), Android $(property ro.build.version.release)" \
    "(API $api), $(property ro.hardware.vulkan) Vulkan driver"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
workspace=$work/workspace
device_root=/data/local/tmp/mln-rs
mkdir -p "$workspace"
tar -xzf "$package/workspace.tar.gz" -C "$workspace"
adb shell "rm -rf $device_root /data/local/tmp/devices-cli && mkdir -p $device_root"

export XDG_STATE_HOME=$work/state
export DEVICES_CLI_OWNER=$workspace
export ANDROID_TEST_HOST=$package/host
export MLN_ANDROID_DEVICE_API=$api
devices-cli assign --owner "$DEVICES_CLI_OWNER" "$ANDROID_SERIAL"

export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUNNER="$workspace/tools/android-test-host/runner.sh arm64-v8a"
export MLN_RENDER_BACKEND=vulkan
export MLN_RENDER_TEST_JOBS=1
export MLN_TEST_OUTPUT=$device_root/test-output
export RUST_BACKTRACE=1

status=0
cargo-nextest nextest run \
    --archive-file "$package/tests.tar.zst" \
    --workspace-remap "$workspace" \
    --profile device-farm \
    --test-threads "$(adb shell nproc | tr -d '\r')" ||
    status=$?

cp "$workspace/target/nextest/device-farm/junit.xml" "$artifacts/" || true
# Without the render suite's images, whose paths start with a test's ID and
# which its reports show for the failed tests, and as one file: adb pulls
# thousands of small files slowly.
if adb shell "test -d $MLN_TEST_OUTPUT"; then
    adb shell "find $MLN_TEST_OUTPUT \\( -path '*/render-tests/*' -o -path '*/query-tests/*' -o -path '*/gljs/*' \\) \
        -name '*.png' -delete > /dev/null; tar -czf $device_root/test-output.tar.gz -C $device_root test-output" &&
        adb pull "$device_root/test-output.tar.gz" "$artifacts/" > /dev/null ||
        echo "Couldn't collect the tests' output" >&2
fi
adb shell "rm -rf /data/local/tmp/devices-cli" || true
exit "$status"
