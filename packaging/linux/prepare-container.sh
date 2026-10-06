#!/usr/bin/env bash
# Installs what build-appimage.sh needs into a fresh ubuntu:22.04 container.
# `just appimage` and the release workflow both run this first.
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
    build-essential ca-certificates curl desktop-file-utils file jq libfontconfig-dev pkg-config
# rust-toolchain.toml in the repo picks the toolchain and components.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none
