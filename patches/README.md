# Patched dependencies

Copies of crates.io packages with a fix that their upstream hasn't released,
which `[patch.crates-io]` in the root Cargo.toml puts in place of the
published package. Each copy is the published package's library and license,
with its change. They aren't part of the workspace, and the formatter
(dprint.jsonc) leaves them as upstream formats them.

Once a release has the fix, update to it and delete the copy and its
`[patch.crates-io]` line. louwers/mln-rs#263 tracks each fix upstream.

- `fsst-rs`: on 32-bit targets, where `u64` aligns to 4 bytes, its hash table
  entry was 12 bytes and failed the crate's assertion that it is 16. The copy
  aligns the entry to 8 bytes.
- `gpu-allocator`: its free-list allocator, which places every Vulkan buffer
  and image, found the best-fitting free chunk by scanning all of a memory
  block's free chunks, and the free chunks grow in number as allocations come
  and go. The copy keeps them ordered by size, so it looks only at chunks large
  enough, smallest first.
