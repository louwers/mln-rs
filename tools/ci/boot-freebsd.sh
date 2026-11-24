#!/usr/bin/env bash
# Prints the SSH host of the FreeBSD virtual machine that tests run on, once it
# has the software graphics drivers and Vulkan validation layer that the suites
# draw with. devices-cli gives this checkout its own machine, booted on demand
# with anyvm from the image that vmactions/freebsd-vm boots, which keeps running
# until `devices-cli release`. The machine mounts the checkout over NFS at the
# path it has on the host, so a test executable, the libraries it loads, and the
# fixtures keep their paths. It needs QEMU, UEFI firmware, and KVM on a Linux
# host.
set -euo pipefail

# The release the suites run on, matching mise.target-freebsd.toml.
release="${MLN_FREEBSD_RELEASE:-14.4}"
if [[ -z "${ANYVM:-}" ]]; then
    anyvm=$(mise where http:anyvm)
    export ANYVM="$anyvm/anyvm.py"
fi
host=$(devices-cli get freebsd --release "$release")
# The software implementations of EGL, OpenGL ES, and Vulkan (plus the Khronos
# validation layer for MLN_VULKAN_VALIDATION=abort). The machine boots from a
# snapshot of the shared image, so it installs them once per boot; pkg returns
# at once when they are already installed.
ssh -o BatchMode=yes "$host" \
    pkg install --yes --quiet \
    mesa-dri mesa-libs libglvnd vulkan-loader vulkan-validation-layers >&2
printf '%s\n' "$host"
