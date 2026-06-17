lint:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test

hw:
    cargo test --features hardware -- --ignored
