#!/bin/sh
# A C compiler for i686-unknown-linux-gnu, for cc-rs (CC_i686_unknown_linux_gnu):
# Zig's clang, with its own glibc headers. It drops the target that cc-rs
# passes as `--target=<rust triple>`, which Zig doesn't parse, for Zig's own.
for arg; do
    shift
    case "$arg" in
        --target=*) ;;
        *) set -- "$@" "$arg" ;;
    esac
done
exec zig cc -target x86-linux-gnu "$@"
