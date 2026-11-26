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

# The release the suites run on, matching
# .config/mise/config.target-freebsd.toml.
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
# anyvm mounts the checkout over its user-space nfsd with NFSv4.0
# (minorversion=0). Under multi-threaded I/O (such as render-test), FreeBSD's
# NFSv4.0 client and nfsd.py desync per-owner seqids (NFS4ERR_BAD_SEQID -> EIO).
# nfsd.py also serves NFSv4.2, which replaces open-owner seqids with session
# slots, so upgrade the mount to minorversion=2 once per boot.
repo_root=$(git rev-parse --show-toplevel)
ssh -o BatchMode=yes "$host" sh -s -- "$repo_root" >&2 <<'EOF'
set -eu
mnt=$1
info=$(nfsstat -m)
opts=$(printf '%s\n' "$info" | awk -v mnt="$mnt" '
    {
        p = index($0, " on ")
        if (p > 0 && substr($0, p + 4) == mnt) {
            if (getline nextline > 0) {
                print nextline
            }
            exit
        }
    }
')
case ",$opts," in
    *,minorversion=0,*)
        src=$(printf '%s\n' "$info" | awk -v mnt="$mnt" '
            {
                p = index($0, " on ")
                if (p > 0 && substr($0, p + 4) == mnt) {
                    print substr($0, 1, p - 1)
                    exit
                }
            }
        ')
        port=$(printf '%s\n' "$opts" | awk '
            match($0, /port=[0-9]+/) {
                print substr($0, RSTART + 5, RLENGTH - 5)
            }
        ')
        if [ -z "$port" ]; then
            host_ip=${src%%:*}
            port=$(sockstat -4 -c -P tcp | awk -v ip="$host_ip" '
                $1 == "?" && index($7, ip ":") == 1 {
                    sub(/^[^:]*:/, "", $7)
                    print $7
                    exit
                }
            ')
        fi
        umount "$mnt"
        mount -t nfs -o "nfsv4,minorversion=2,tcp,port=${port:-2049}" "$src" "$mnt"
        ;;
esac
EOF
printf '%s\n' "$host"
