# m2k_cli: the ADALM2000 CLI and its iiod client.

default:
    @just --list

# Tests of the workspace (the emulator; device tests need the M2K)
test:
    cargo test --workspace --quiet

# Format check and clippy without warnings
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# Build and install m2k on this PC; completion: source <(COMPLETE=bash m2k)
install:
    cargo install --quiet --path . --locked

# Bring this PC up to main
deploy:
    tpm repos pull m2k_cli
    just install
