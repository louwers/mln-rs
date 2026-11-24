#!/usr/bin/env bash
# cargo-nextest's target runner on an AWS Device Farm test host
# (run-tests.sh): runs a test binary on the job's phone, as devices-cli does on
# an emulator, but inside a Java VM's process, as an app's code runs: the test
# host (host/) loads the binary into a process that app_process started, and
# calls its main. The binary goes to bin/ under $DEVICE_FARM_ROOT once, and
# runs in its package's copy of the workspace, which run-tests.sh made there,
# with the RUST_* and MLN_* variables and CARGO_MANIFEST_DIR, and with host
# workspace paths in them and in the arguments made the copy's.
#
# Files go over adb's sync protocol (adb push): Device Farm's adb garbles what
# `adb shell` reads from stdin, so the program gets none.
set -euo pipefail

program=$1
shift
root=${DEVICE_FARM_ROOT:?}
workspace=${DEVICE_FARM_WORKSPACE:?}
pushed=${DEVICE_FARM_PUSHED:?}

name=${program##*/}
device_program=$root/bin/$name
# nextest lists and runs a binary's tests in parallel: one pushes it.
exec 9> "$pushed/$name.lock"
flock 9
if [[ ! -e $pushed/$name ]]; then
    adb push "$program" "$device_program" > /dev/null
    touch "$pushed/$name"
fi
exec 9>&-

on_device() {
    case $1 in
        "$workspace" | "$workspace"/*) printf '%s' "$root/workspace${1#"$workspace"}" ;;
        *) printf '%s' "$1" ;;
    esac
}
quote() {
    printf "'%s'" "${1//\'/\'\\\'\'}"
}

script="cd $(quote "$(on_device "$PWD")") && exec env"
while IFS= read -r variable; do
    case $variable in
        RUST_* | MLN_* | CARGO_MANIFEST_DIR) script+=" $(quote "$variable=$(on_device "${!variable}")")" ;;
    esac
done < <(compgen -e)
script+=" CLASSPATH=$root/test-host.jar app_process /system/bin org.maplibre.mlnrs.TestHost"
script+=" $root/libmln_test_host.so $(quote "$device_program")"
for argument; do
    script+=" $(quote "$(on_device "$argument")")"
done
exec adb shell "$script" < /dev/null
