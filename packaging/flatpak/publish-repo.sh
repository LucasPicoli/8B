#!/usr/bin/env bash
# Import release bundles into one signed OSTree repo that Flatpak can use as a remote.
# Usage: publish-repo.sh KEY_ID OUT_REPO BUNDLE...
# The signing key must already be usable by gpg, with its passphrase cached if it has one.
set -euo pipefail

key=${1:?usage: publish-repo.sh KEY_ID OUT_REPO BUNDLE...}
out=${2:?usage: publish-repo.sh KEY_ID OUT_REPO BUNDLE...}
shift 2
[ $# -gt 0 ] || { echo "no bundles given" >&2; exit 2; }

ostree --repo="$out" init --mode=archive-z2
for bundle in "$@"; do
  flatpak build-import-bundle --gpg-sign="$key" "$out" "$bundle"
done
# Writes the signed summary and the appstream branch.
flatpak build-update-repo --gpg-sign="$key" "$out"
