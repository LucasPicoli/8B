lint:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test
    # Alone, so a test that needs a feature only another crate turns on fails here.
    cargo test -p controller-core

hw:
    cargo test --features hardware -- --ignored

# Build the x86_64 AppImage in ubuntu:22.04 (glibc 2.35), so it runs on that glibc and newer.
appimage:
    $(command -v podman || command -v docker) run --rm -v "{{justfile_directory()}}:/src" -w /src ubuntu:22.04 \
        bash -c 'packaging/linux/prepare-container.sh && packaging/linux/build-appimage.sh x86_64'

# Build 8B-<version>-<arch>.flatpak and its .sha256 into packaging/flatpak/out/. Needs flatpak-builder and uv.
flatpak:
    #!/usr/bin/env bash
    set -euo pipefail
    cd packaging/flatpak
    ./cargo-sources.sh
    version=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "gui") | .version')
    arch=$(uname -m)
    bundle=8B-$version-$arch.flatpak
    mkdir -p out
    flatpak-builder --user --install-deps-from=flathub --force-clean --disable-rofiles-fuse \
        --state-dir=out/state --repo=out/repo --default-branch=stable out/build io.github.LucasPicoli._8B.yml
    flatpak build-bundle out/repo "out/$bundle" --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
        --arch="$arch" io.github.LucasPicoli._8B stable
    (cd out && sha256sum "$bundle" > "$bundle.sha256")
    ls -l out/"$bundle" out/"$bundle.sha256"
