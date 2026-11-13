# Patched dependencies

Copies of crates.io packages with a fix that their upstream hasn't released,
which `[patch.crates-io]` in the root Cargo.toml puts in place of the
published package. Each copy is the published package's library and license,
with its change. They aren't part of the workspace, and `mise run fmt` leaves them
as upstream formats them.

Once a release has the fix, update to it and delete the copy and its
`[patch.crates-io]` line. louwers/mln-rs#263 tracks each fix upstream.

- `fsst-rs`: on 32-bit targets, where `u64` aligns to 4 bytes, its hash table
  entry was 12 bytes and failed the crate's assertion that it is 16. The copy
  aligns the entry to 8 bytes.
