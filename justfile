lint:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test

hw:
    cargo test --features hardware -- --ignored

# Build the x86_64 AppImage in ubuntu:22.04 (glibc 2.35), so it runs on that glibc and newer.
appimage:
    $(command -v podman || command -v docker) run --rm -v "{{justfile_directory()}}:/src" -w /src ubuntu:22.04 \
        bash -c 'packaging/linux/prepare-container.sh && packaging/linux/build-appimage.sh x86_64'
