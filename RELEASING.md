# Releasing

The root `Cargo.toml` is the workspace that goes to crates.io, all at one
version:

- **mln-rs**, the package in the repository root. It re-exports the libraries
  under their crate names: `mln_rs::mln_engine`, `mln_rs::mln_style`,
  `mln_rs::geometry`, and so on. Users depend on this one.
- **`mln-rs-<name>`**, one package per library under `crates/` and `vendor/`
  (for example `mln-rs-engine` for `mln_engine`, `mln-rs-geometry` for
  `geometry`). Their library names are unchanged, so the code still says
  `mln_engine::`. They exist for mln-rs only.

The `publish = false` packages (the desktop app, the browser build in
examples/web and crates/mln_web_shaper, the test tools and their headless
frontend) are not released.

## Releasing a version

Every night, and when run by hand (Actions → release-pr → Run workflow), the
release-pr workflow has [release-plz](https://release-plz.dev)
(`release-plz.toml`) open a pull request for the next release,
`Release v<version>`, or update the one it opened. It changes:

- the version in `Cargo.toml`: `[workspace.package] version` and the
  `=<version>` of every in-repo entry in `[workspace.dependencies]`, and
  `Cargo.lock`;
- `CHANGELOG.md`: the subjects of the commits since the last release that
  changed a published package, newest first.

[cargo-semver-checks](https://github.com/obi1kenobi/cargo-semver-checks) picks
the version: it compares each library with its release on crates.io. A change
that breaks a library's API bumps the minor version (0.1.3 to 0.2.0), and any
other change the patch (0.1.3 to 0.1.4). The packages share their version, so
they all take the largest bump. The pull request lists the packages whose API
broke.

cargo-semver-checks builds the libraries for Linux, so it doesn't see what only
other targets build: all of mln-rs-mtl (Apple platforms) and mln-rs-web-host
(the browser), and any item behind a `cfg` for another platform. It doesn't
catch every break either. For a break it misses, set the version by hand
(below).

To release, merge the pull request. The release workflow then publishes the
packages, tags the commit `v<version>` and creates a GitHub release with the
version's part of `CHANGELOG.md`. crates.io never replaces a published version.

release-plz updates the pull request only at night. If anything merged after
that, run release-pr by hand and wait for it to update the pull request before
you merge it: otherwise those changes go out under a version cargo-semver-checks
didn't check them against.

Before merging, you can edit the changelog, or set another version on the pull
request's branch with `release-plz set-version <version>`, which also updates
the `=<version>` entries, `Cargo.lock` and the changelog. Then merge it before
release-plz runs again, at night or by hand: it replaces a release pull request
that has commits besides its own with a new one.

### Releasing 0.1.0

From a 0.0.x version, release-plz bumps only the patch, breaking change or not:
it leaves the step to 0.1.0 to the maintainers. Take it on the first release
pull request, which proposes 0.0.12: run `release-plz set-version 0.1.0` on its
branch, push, and merge it. From 0.1.0 on, a patch release is compatible, so a
dependency on `mln-rs = "0.1"` takes it.

## When a release fails

If the release workflow fails part way, the next push to main runs it again, and
so do Re-run jobs on the failed run, `mise run release` and Actions → release →
Run workflow. Each run releases the commit that set the version, normally the
release pull request's merge, whatever commit it runs on. Packages that
crates.io already has at this version are skipped, and so is an existing GitHub
release. If the fix changes a package, release a new version instead.

crates.io limits how many new crates an account can publish in a short time. A
release that adds more libraries than that stops with status 429: run it again
later, and it continues. To avoid the wait, ask help@crates.io to raise the
limit first.

## What the workflows do

`.github/workflows/release.yml` runs on every push to main, and when run by
hand. In a fork, it does nothing.

1. `version` checks whether the version in `Cargo.toml` has its GitHub release,
   which `release` creates last. It checks out only `Cargo.toml` and builds
   nothing.
2. If it doesn't, or the workflow was run by hand, `release`:
   1. Checks out the commit that set the version: the latest on the branch whose
      change to `Cargo.toml` adds the version's line.
   2. Installs stable Rust and runs `cargo package --workspace --locked`
      (without the desktop app). It packages every crate and builds each one
      from its packaged sources. The packages resolve each other locally, so
      this works before any of them is on crates.io.
   3. Publishes the packages crates.io does not have at this version, one at a
      time in dependency order (`cargo publish --package …`). A crate that
      requires Trusted Publishing gets a token that crates.io trades for the
      job's GitHub OIDC token, and any other, such as a new crate,
      `CARGO_REGISTRY_TOKEN`.
   4. Creates the tag `v<version>` on that commit and a GitHub release with the
      `.crate` files and the version's part of `CHANGELOG.md`.

`.github/workflows/release-pr.yml` runs `release-plz release-pr` on the default
branch's latest commit, which opens or updates the release pull request. It runs
at 02:13 UTC and when run by hand, not on every push, since release-plz runs
cargo-semver-checks on every library that changed since the last release. In a
fork, it does nothing.

CI (`package` in `ci.yml`, `mise run ci:package`) runs step 2.2 on every push to
main and on pull requests that Rubberstamp plans `package` for (manifest changes
or added, deleted, or renamed files). It catches a manifest crates.io would
reject.

## Locally

```sh
mise run package
```

On NixOS, run it inside `nix develop` if a build script cannot find a C
compiler.

`release-plz update` makes the release pull request's changes in your checkout.
With cargo-semver-checks installed
(`cargo install --locked release-plz cargo-semver-checks`), it also checks the
APIs and prints what broke. Run it in a clone: in a linked worktree, it checks
out another branch.

## Adding a library

A new library under `crates/` or `vendor/` needs:

- a `Cargo.toml`: copy one from another library and set the package name
  (`mln-rs-<name>`), `[lib] name` and the dependencies (as
  `<crate>.workspace = true`). Its tests: a `[[test]]` per integration test, and
  their extra dependencies in `[dev-dependencies]`, in-repo ones by path without
  a version (`mln_style = { package = "mln-rs-style", path = "../mln_style" }`):
  Cargo drops those from the published package, so they may form cycles;
- a `README.md`, its page on crates.io: copy one from another library, keep the
  warning that it is an internal crate (with its `mln_rs::<name>` path) and say
  what it ports. Cargo packages it without a `readme` key;
- an entry in `[workspace.dependencies]` in the root `Cargo.toml`;
- an entry in mln-rs's `changelog_include` in `release-plz.toml`, so that its
  commits are in the changelog;
- a dependency of mln-rs (root `[dependencies]`) and a `pub use` in
  `src/lib.rs`.

Its first release publishes it with `CARGO_REGISTRY_TOKEN`. Then a crate owner
runs `mise run release:setup-trusted-publishing`
([Secrets and settings](#secrets-and-settings)).

A library that should not be released gets `publish = false` instead of the
README and the last three steps. A new third-party crate goes into
`[workspace.dependencies]` only.

## Secrets and settings

- **Trusted Publishing** (each crate's Settings on crates.io): a GitHub
  publisher with the repository `maplibre/mln-rs`, the workflow `release.yml`
  and the environment `release`, and "Require trusted publishing for all new
  versions", so that no API token can publish the crate.
  `mise run release:setup-trusted-publishing` sets both on every crate a release
  publishes that is on crates.io. It takes a crate owner's API token with the
  `trusted-publishing` scope. Run it after a release that publishes a new crate.
  `cargo owner --add <user>` invites another owner.
- **`CARGO_REGISTRY_TOKEN`** (repository secret): a crates.io API token of a
  crate owner, for the crates that don't require Trusted Publishing, such as new
  ones. It needs the `publish-new` and `publish-update` scopes, and can be
  limited to the crates `mln-rs*`. crates.io tokens can expire, so renew it when
  it does. The account needs a verified email address to publish.
- **The `release` environment** (Settings → Environments): its deployment
  branches limited to the default branch. Trusted Publishing trusts any job that
  runs in the environment, including one from a branch that edits the workflow.
- **Allow GitHub Actions to create and approve pull requests** (Settings →
  Actions → General → Workflow permissions): without it, the workflow's default
  token can't open the release pull request.
- **`RELEASE_PLZ_TOKEN`** (repository secret, optional): a pull request that the
  default token opens starts no workflows, so CI doesn't run on the release pull
  request until someone closes and reopens it. A fine-grained personal access
  token here, with read and write access to this repository's contents and pull
  requests, opens it instead, as the token's account, and CI runs.
  (maplibre/martin uses a GitHub App, whose token a step creates on each run.)
- **Workflow permissions**: `release` asks for `contents: write` to push the tag
  and create the release, and release-pr.yml for `contents: write` and
  `pull-requests: write` to push the pull request's branch and open it. That
  works with the default repository setting (Settings → Actions → General). An
  organization or enterprise policy that caps `GITHUB_TOKEN` at read-only would
  block it.
- **Tag rules**: a ruleset or tag protection that covers `v*` must let GitHub
  Actions create tags.
- The workflow refuses to release from any branch other than the default branch.
