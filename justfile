# m2k: the ADALM2000 library, CLI and Python module, and the iiod client.

py_venv := "/tmp/m2k_py-venv"

default:
    @just --list

# The tests. `just test`: all of them (test-all), the full run used before merging to main. `just test --changed`:
# only the workspace crates changed against main and the crates that depend on them, what to run before
# marking work ready for review.
test *flags:
    {{ if flags == "--changed" { "changedtests run" } else { just_executable() + " test-all " + flags } }}

# Tests of the workspace (the emulator; device tests need the M2K) and the Python smoke test
test-all: test-py
    cargo test --workspace --quiet

# Build the Python module (crates/m2k_py) into a venv under /tmp and run its pytest smoke test
[working-directory('crates/m2k_py')]
test-py:
    UV_PROJECT_ENVIRONMENT={{py_venv}} uv run --quiet --group dev pytest -q

# Format check and clippy without warnings
lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

# With tpm on PATH: tpm install (kept-releases stack, one copy on PATH); otherwise: cargo install
# Build and install m2k on this PC; completion: source <(COMPLETE=bash m2k)
install:
    cargo build --profile deploy --locked -p m2k_cli
    if command -v tpm >/dev/null; then tpm install target/deploy/m2k; else cargo install --locked --path crates/m2k_cli; fi

# Bring this PC up to main: pull, install, check --version against main
deploy:
    if command -v tpm >/dev/null; then tpm repos pull m2k; else git pull --ff-only; fi
    just install
    m2k --version | grep -q "($(git rev-parse --short HEAD)" || { echo "m2k --version does not show $(git rev-parse --short HEAD)"; m2k --version; exit 1; }
    m2k --version
