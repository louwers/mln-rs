#!/usr/bin/env bash
# Cargo's target runner for Android (.config/mise/config.target-android.toml,
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

    # The test host, named for its sources, links to a finished build: one
    # that has its launcher, which build.sh writes last. A runner that finds
    # none builds one, and links to it unless another runner linked to a
    # finished build meanwhile, by renaming a new link over the old one, so
    # the name always resolves and no build that a link named goes away. The
    # old link may name an emptied build: rust-cache, in CI, keeps the link
    # but deletes the build's files.
    key=$(cat "$here/build.sh" "$here/launcher.sh" "$here/test-host.c" "$here/org/maplibre/mlnrs/TestHost.java" |
        cksum | cut -d ' ' -f 1)
    host=$out/$abi-$key
    if [[ ! -e $host/launcher.sh ]]; then
        build=$(mktemp -d "$out/build.XXXXXX")
        "$here/build.sh" "$abi" "$build"
        if [[ -e $host/launcher.sh ]]; then
            rm -rf "$build"
        else
            link=$build.link
            ln -s "${build##*/}" "$link"
            # GNU mv takes -T, BSD mv -h, to replace the link itself.
            mv -T "$link" "$host" 2> /dev/null || mv -h "$link" "$host"
        fi
    fi
fi

export ANDROID_TEST_HOST=$host
args=(exec android --api "${MLN_ANDROID_DEVICE_API:-36}" --abi "$abi")
if [[ -n ${DEVICES_CLI_OWNER:-} ]]; then
    args+=(--owner "$DEVICES_CLI_OWNER")
fi
args+=(
    --sync-env CARGO_MANIFEST_DIR --sync-env ANDROID_TEST_HOST --sync "$program"
    --env 'RUST_*' --env 'MLN_*' --env 'WGPU_*'
    -- "$host/launcher.sh" "$program" "$@"
)
exec devices-cli "${args[@]}" < /dev/null
