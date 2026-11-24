#!/usr/bin/env bash
# Boots this checkout's Android emulator for CI (x86_64, at the API level that
# tools/android-test-host/runner.sh runs the tests on) via devices-cli, in the
# target-android environment, updating the emulator first when needed, and
# verifies its ABI and API level over adb.
set -euo pipefail

api=${MLN_ANDROID_DEVICE_API:-36}
abi=x86_64
sdk_root="${ANDROID_HOME:?ANDROID_HOME must point at an Android SDK}"
adb="$sdk_root/platform-tools/adb"

# Emulators before this revision can hang for good when a guest process exits
# while it still owns GLES objects (gfxstream b/561516389). Until the stable
# channel carries the fix, the update comes from the dev channel. devices-cli
# installs the emulator only when the SDK has none.
minimum_emulator_version=37.3.5
emulator_revision=$(sed -n 's/^Pkg\.Revision=//p' "$sdk_root/emulator/source.properties" 2> /dev/null | tr -d '\r') || true
if [[ "$(printf '%s\n' "$minimum_emulator_version" "$emulator_revision" | sort -V | head -n1)" != "$minimum_emulator_version" ]]; then
    echo "Updating the Android emulator from revision ${emulator_revision:-<none>} to $minimum_emulator_version or later." >&2
    # The android CLI's canary packages include the dev channel.
    android --sdk="$sdk_root" sdk install --canary emulator <<< "y" >&2
fi

# SwiftShader draws the GLES and Vulkan targets in software, which is what a
# runner without a GPU has. The emulator's renderer makes JIT code executable
# in memory that its bundled tcmalloc takes with sbrk, which SELinux refuses
# as execheap on Fedora; memory that tcmalloc maps instead needs only execmem.
serial=$(TCMALLOC_SKIP_SBRK=true devices-cli get android --api "$api" --abi "$abi" -- \
    -gpu swiftshader -memory 4096 -camera-back none -camera-front none)

# A call to a device that is still settling can block indefinitely, and an adb
# server that has just started knows no device until its first scan.
bounded_adb() {
    local status=0
    perl -e 'alarm shift; exec @ARGV' 30 "$adb" -s "$serial" "$@" || status=$?
    if ((status)); then
        echo "adb exited $status during $* on Android device $serial." >&2
        exit "$status"
    fi
}

bounded_adb wait-for-device
device_abi=$(bounded_adb shell getprop ro.product.cpu.abi)
device_api=$(bounded_adb shell getprop ro.build.version.sdk)
device_abi=${device_abi//$'\r'/}
device_api=${device_api//$'\r'/}
if [[ "$device_abi" != "$abi" || "$device_api" != "$api" ]]; then
    echo "Android device $serial runs ${device_abi:-an unknown ABI} at API ${device_api:-unknown}; the tests need $abi at API $api." >&2
    exit 1
fi
printf '%s\n' "$serial"
