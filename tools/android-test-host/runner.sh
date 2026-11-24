#!/usr/bin/env bash
# Cargo's target runner for Android (mise.target-android.toml,
# tools/device-farm): runs a test executable on this checkout's emulator, or a
# device assigned to it, through devices-cli, in a Java VM's process
# (README.md). devices-cli copies the executable, the test host and the test's
# package to the device once, and again when they change.
#
# When ANDROID_TEST_HOST is set, uses that prebuilt test host instead of
# building one (used on AWS Device Farm, which has no JDK or NDK).
#
# Usage: runner.sh <abi> <executable> [arguments]
set -euo pipefail

abi=$1
program=$2
shift 2
program=$(cd "$(dirname "$program")" && pwd)/$(basename "$program")
here=$(cd "$(dirname "$0")" && pwd)

if [[ -n ${ANDROID_TEST_HOST:-} ]]; then
    host=$(cd "$ANDROID_TEST_HOST" && pwd)
else
    out=${CARGO_TARGET_DIR:-$(cd "$here/../.." && pwd)/target}/android-test-host
    mkdir -p "$out"

    # The test host, named for its sources: concurrent tests build it once, an
    # edit builds it again, and the link to a finished build is made atomically.
    key=$(cat "$here/build.sh" "$here/launcher.sh" "$here/test-host.c" "$here/org/maplibre/mlnrs/TestHost.java" |
        cksum | cut -d ' ' -f 1)
    host=$out/$abi-$key
    if [[ ! -e $host ]]; then
        build=$(mktemp -d "$out/build.XXXXXX")
        "$here/build.sh" "$abi" "$build"
        ln -sn "${build##*/}" "$host" 2> /dev/null || rm -rf "$build"
    fi
fi

export ANDROID_TEST_HOST=$host
args=(exec android --api "${MLN_ANDROID_DEVICE_API:-36}" --abi "$abi")
if [[ -n ${DEVICES_CLI_OWNER:-} ]]; then
    args+=(--owner "$DEVICES_CLI_OWNER")
fi
args+=(
    --sync-env CARGO_MANIFEST_DIR --sync-env ANDROID_TEST_HOST --sync "$program"
    --env 'RUST_*' --env 'MLN_*'
    -- "$host/launcher.sh" "$program" "$@"
)
exec devices-cli "${args[@]}" < /dev/null
