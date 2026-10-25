# Common tasks. `just` lists them.

default:
    @just --list

build:
    cargo build --workspace

release:
    cargo build --workspace --release

# Start the desktop app.
run *args:
    cargo run -p sim-gui -- {{args}}

# Run the command-line tool, e.g. `just cli check tests/fixtures/v13-model.bpmn`.
cli *args:
    cargo run -p sim-cli -- {{args}}

# Serve the browser version on http://127.0.0.1:8080, rebuilt on every change.
web:
    cd crates/sim-gui && trunk serve

# Build the browser version into target/web.
web-build *args:
    cd crates/sim-gui && trunk build --release {{args}}

test:
    cargo test --workspace

watch:
    bacon clippy-all

fmt:
    cargo fmt --all

lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo clippy -p sim-gui --target wasm32-unknown-unknown -- -D warnings

clean:
    cargo clean
