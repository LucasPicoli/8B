#!/usr/bin/env bash
# Writes packaging/flatpak/cargo-sources.json (the offline crate list) from Cargo.lock.
# `just flatpak` and the release workflow both run this before flatpak-builder.
set -euo pipefail

# Bump the commit and the checksum together.
GENERATOR_URL=https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/74697c75b630d7330e77250fc13cb5ea688d9479/cargo/flatpak-cargo-generator.py
GENERATOR_SHA256=0a2db6be87d75910facef28ab46d4d6460802e8419ab850d0caa6a364d26b380

here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl --proto '=https' --tlsv1.2 -sSfL -o "$tmp/generator.py" "$GENERATOR_URL"
echo "$GENERATOR_SHA256  $tmp/generator.py" | sha256sum -c --quiet -
# The script's PEP 723 header lists its Python dependencies; uv installs them.
uv run "$tmp/generator.py" "$here/../../Cargo.lock" -o "$here/cargo-sources.json"
