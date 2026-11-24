#!/usr/bin/env bash
# Boots this checkout's Android emulator for CI (API 36, x86_64) via
# devices-cli, updating the emulator first when needed, and verifies its ABI
# and API level over adb.
set -euo pipefail

api=36
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
    # Google's android CLI needs no JDK, and its canary packages include the
    # dev channel. sdkmanager, which runs on a JVM, serves command-line tools
    # that predate it.
    install=()
    if android_bin=$(command -v android 2> /dev/null); then
        install=("$android_bin" --sdk="$sdk_root" sdk install --canary emulator)
    else
        for tool in android sdkmanager; do
            for candidate in "$sdk_root"/cmdline-tools/latest/bin/"$tool" "$sdk_root"/cmdline-tools/*/bin/"$tool"; do
                [[ -x "$candidate" ]] || continue
                if [[ "$tool" == android ]]; then
                    install=("$candidate" --sdk="$sdk_root" sdk install --canary emulator)
                else
                    install=("$candidate" --sdk_root="$sdk_root" --channel=2 --install emulator)
                fi
                break 2
            done
        done
    fi
    if ((${#install[@]} == 0)); then
        echo "The Android emulator is at revision ${emulator_revision:-<none>}, below $minimum_emulator_version, and neither PATH nor $sdk_root has an android CLI or sdkmanager to update it." >&2
        exit 1
    fi
    echo "Updating the Android emulator from revision ${emulator_revision:-<none>} to $minimum_emulator_version or later." >&2
    "${install[@]}" <<< "y" >&2
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
