#!/usr/bin/env bash
# Cargo's target runner for Android (mise.target-android.toml): runs a test
# executable on this checkout's emulator, or a device assigned to it, through
# devices-cli, in a Java VM's process (README.md). devices-cli copies the
# executable, the test host and the test's package to the device once, and
# again when they change.
#
# Usage: runner.sh <abi> <executable> [arguments]
set -euo pipefail

abi=$1
program=$2
shift 2
here=$(cd "$(dirname "$0")" && pwd)
out=${CARGO_TARGET_DIR:-$(cd "$here/../.." && pwd)/target}/android-test-host
mkdir -p "$out/bin"

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

# The executable alone in a directory, for devices-cli to copy, replaced
# atomically, since its tests run concurrently.
name=${program##*/}
copy=$out/bin/$name/$name
mkdir -p "${copy%/*}"
if [[ ! $copy -ef $program ]]; then
    link=$(mktemp "$copy.XXXXXX")
    ln -f "$program" "$link" 2> /dev/null || cp -p "$program" "$link"
    # mv refuses when a concurrent test has linked the same file there.
    mv -f "$link" "$copy" 2> /dev/null || rm -f "$link"
fi

export ANDROID_TEST_HOST=$host
exec devices-cli exec android --api 36 --abi "$abi" \
    --sync-env CARGO_MANIFEST_DIR --sync-env ANDROID_TEST_HOST --sync "${copy%/*}" --env 'RUST_*' --env 'MLN_*' \
    -- "$host/launcher.sh" "$copy" "$@"
