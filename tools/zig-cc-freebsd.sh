#!/bin/sh
# A C compiler and linker for x86_64-unknown-freebsd, for cc-rs
# (CC_x86_64_unknown_freebsd) and Cargo: Zig's clang, with its own FreeBSD libc
# headers and the libraries that Zig lacks, such as libkvm, from the base
# system in MLN_FREEBSD_SYSROOT. It drops the target that cc-rs passes as
# `--target=<rust triple>`, which Zig doesn't parse, for Zig's own.
for arg; do
    shift
    case "$arg" in
        --target=*) ;;
        *) set -- "$@" "$arg" ;;
    esac
done
exec zig cc -target x86_64-freebsd -L "$MLN_FREEBSD_SYSROOT/usr/lib" -L "$MLN_FREEBSD_SYSROOT/lib" "$@"
