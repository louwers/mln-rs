#!/bin/sh
# A C compiler for wasm32-unknown-unknown, for cc-rs (CC_wasm32_unknown_unknown):
# Zig's clang, which targets wasm32 wherever it runs, where Apple's clang
# doesn't. It drops the target that cc-rs passes as `--target=<rust triple>`,
# which Zig doesn't parse, for Zig's own.
for arg; do
    shift
    case "$arg" in
        --target=*) ;;
        *) set -- "$@" "$arg" ;;
    esac
done
exec zig cc -target wasm32-freestanding "$@"
