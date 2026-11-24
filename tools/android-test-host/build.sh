#!/usr/bin/env bash
# Builds the test host for an Android ABI into a directory: TestHost.java as
# dex for app_process (test-host.jar), its native half
# (libmln_test_host.so) and launcher.sh. In the target-android environment,
# which has the JDK, the build-tools' d8 and the NDK's compilers.
#
# Usage: build.sh <abi> <directory>
set -euo pipefail

abi=$1
out=$2
here=$(cd "$(dirname "$0")" && pwd)
case $abi in
    arm64-v8a) cc=${CC_aarch64_linux_android:?} ;;
    x86_64) cc=${CC_x86_64_linux_android:?} ;;
    *)
        echo "build.sh: no test host for $abi" >&2
        exit 1
        ;;
esac

classes=$(mktemp -d)
trap 'rm -rf "$classes"' EXIT
mkdir -p "$out"
javac --release 17 -d "$classes" "$here/org/maplibre/mlnrs/TestHost.java"
java -cp "${ANDROID_HOME:?}/build-tools/${MLN_ANDROID_BUILD_TOOLS:?}/lib/d8.jar" com.android.tools.r8.D8 \
    --min-api "${MLN_ANDROID_API:?}" --output "$out/test-host.jar" "$classes/org/maplibre/mlnrs/TestHost.class"
"$cc" -shared -fPIC -O2 -Wall -Wextra -Werror "$here/test-host.c" -o "$out/libmln_test_host.so"
cp "$here/launcher.sh" "$out/"
