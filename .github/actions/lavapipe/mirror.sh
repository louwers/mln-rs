#!/usr/bin/env bash
# Uploads the packages that packages.sha256 and packages-egl.sha256 pin, each
# a SHA-256 and a path in the Ubuntu archive, to the release that action.yml
# installs them from. Run it after changing the pins, with a `gh` that may
# write to the repository. Packages the release already has stay, for the
# branches that still pin them.
set -euo pipefail

repo=louwers/mln-rs
tag=ci-lavapipe

here=$(dirname "${BASH_SOURCE[0]}")
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT

debs=()
while read -r sha256 path; do
  deb="$dir/${path##*/}"
  curl -fsSL --retry 5 -o "$deb" "https://archive.ubuntu.com/ubuntu/$path"
  echo "$sha256  $deb" | sha256sum --check --quiet
  debs+=("$deb")
done < <(cat "$here/packages.sha256" "$here/packages-egl.sha256")

if ! gh release view "$tag" --repo "$repo" > /dev/null 2>&1; then
  gh release create "$tag" --repo "$repo" --prerelease \
    --title "Mesa for CI" \
    --notes "Ubuntu's Mesa packages (the Vulkan drivers and EGL), unmodified, for CI to install (.github/actions/lavapipe). Not a release of mln-rs."
fi

# Only the packages the release lacks: a package's name has its version.
have=$(gh release view "$tag" --repo "$repo" --json assets --jq '.assets[].name')
new=()
for deb in "${debs[@]}"; do
  grep -qxF "${deb##*/}" <<< "$have" || new+=("$deb")
done
if [ "${#new[@]}" -gt 0 ]; then
  gh release upload "$tag" --repo "$repo" "${new[@]}"
fi
