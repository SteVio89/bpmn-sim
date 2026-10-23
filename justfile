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

test:
    cargo test --workspace

watch:
    bacon clippy-all

fmt:
    cargo fmt --all

lint:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

clean:
    cargo clean
