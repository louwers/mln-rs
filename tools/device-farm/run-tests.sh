#!/usr/bin/env bash
# Runs the Android tests on the phone of an AWS Device Farm job: android.yml
# runs it on the job's test host, from the test package that
# `mise run device-farm:package` builds (README.md). cargo-nextest runs the
# package's archive of the tests with its device-farm profile, each test on the
# phone in a Java VM's process through runner.sh, on Vulkan. The job's
# artifacts get nextest's JUnit report and the render suite's report and
# images. Exits as nextest does.
set -euo pipefail

package=${DEVICEFARM_TEST_PACKAGE_PATH:?}
artifacts=${DEVICEFARM_LOG_DIR:?}
export ANDROID_SERIAL=${DEVICEFARM_DEVICE_UDID:?}

property() {
    adb shell getprop "$1" | tr -d '\r'
}
echo "$(property ro.product.manufacturer) $(property ro.product.model), Android $(property ro.build.version.release)" \
    "(API $(property ro.build.version.sdk)), $(property ro.hardware.vulkan) Vulkan driver"

work=$(mktemp -d)
export DEVICE_FARM_ROOT=/data/local/tmp/mln-rs
export DEVICE_FARM_WORKSPACE=$work/workspace
export DEVICE_FARM_PUSHED=$work/pushed
mkdir "$DEVICE_FARM_WORKSPACE" "$DEVICE_FARM_PUSHED" "$work/archive"
tar -xzf "$package/workspace.tar.gz" -C "$DEVICE_FARM_WORKSPACE"
adb shell "rm -rf $DEVICE_FARM_ROOT && mkdir -p $DEVICE_FARM_ROOT/bin $DEVICE_FARM_ROOT/workspace"
adb push "$package/test-host.jar" "$package/libmln_test_host.so" "$DEVICE_FARM_ROOT/" > /dev/null
adb push "$package/workspace.tar.gz" "$DEVICE_FARM_ROOT/workspace.tar.gz" > /dev/null
adb shell "tar -xzf $DEVICE_FARM_ROOT/workspace.tar.gz -C $DEVICE_FARM_ROOT/workspace && rm $DEVICE_FARM_ROOT/workspace.tar.gz"

export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUNNER=$package/runner.sh
export MLN_RENDER_BACKEND=vulkan
export MLN_RENDER_TEST_OUTPUT=$DEVICE_FARM_ROOT/render-tests
export RUST_BACKTRACE=1

status=0
"$package/bin/cargo-nextest" nextest run \
    --archive-file "$package/tests.tar.zst" \
    --workspace-remap "$DEVICE_FARM_WORKSPACE" \
    --extract-to "$work/archive" \
    --profile device-farm \
    --test-threads "$(adb shell nproc | tr -d '\r')" ||
    status=$?

cp "$DEVICE_FARM_WORKSPACE/target/nextest/device-farm/junit.xml" "$artifacts/" || true
# As one file: adb pulls thousands of small files slowly.
if adb shell "test -d $MLN_RENDER_TEST_OUTPUT"; then
    adb shell "tar -czf $DEVICE_FARM_ROOT/render-tests.tar.gz -C $DEVICE_FARM_ROOT render-tests"
    adb pull "$DEVICE_FARM_ROOT/render-tests.tar.gz" "$artifacts/" > /dev/null
fi
exit "$status"
