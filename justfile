# m2k_cli: the ADALM2000 library, CLI and Python module, and the iiod client.

py_venv := "/tmp/m2k_py-venv"

default:
    @just --list

# Tests of the workspace (the emulator; device tests need the M2K) and the Python smoke test
test: test-py
    cargo test --workspace --quiet

# Build the Python module (crates/m2k_py) into a venv under /tmp and run its pytest smoke test
[working-directory('crates/m2k_py')]
test-py:
    UV_PROJECT_ENVIRONMENT={{py_venv}} uv run --quiet --group dev pytest -q

# Format check and clippy without warnings
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# Build and install m2k on this PC; completion: source <(COMPLETE=bash m2k)
install:
    cargo install --quiet --path crates/m2k_cli --locked

# Bring this PC up to main
deploy:
    tpm repos pull m2k_cli
    just install
